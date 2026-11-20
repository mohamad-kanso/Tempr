//! `IntelligenceService` owns the `SemanticEngine` and sits between the event
//! bus, the schema service and the UI (docs/05-services.md, spec §6).
//!
//! Completion and hover are synchronous calls on the UI thread. Diagnostics
//! take an immutable text snapshot after the view's idle debounce; the service
//! keeps one mirror `Buffer` per file, caches the analysis by source revision
//! and publishes `DiagnosticsReady`.
//!
//! Catalog lifecycle is event-driven: `SchemaRefreshed` rebuilds the
//! `CatalogCache`, and a failed refresh leaves the previous catalog in place.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use parking_lot::{Mutex, RwLock};
use tempr_domain::{
    ConnectionId, ConnectionState, PluginCompletionRequest, SchemaSnapshot, SchemaSnapshotId,
    SqlFileId,
};
use tempr_editor::Buffer;
use tempr_events::{AppEvent, AppEventKind, EventBus, EventFilter, Subscription};
use tempr_intel::completion::COMPLETION_LIMIT;
use tempr_intel::{
    Analysis, CatalogCache, CompletionItem, CompletionRequest, HoverInfo, SemanticEngine,
    match_quality, prefix_before,
};
use tempr_plugin::{CompletionProvider, PLUGIN_PRIORITY_MAX};
use tokio::runtime::Handle;

use crate::schema::SchemaService;
use crate::{Service, ServiceError};

struct StoredAnalysis {
    source_revision: u64,
    analysis: Arc<Analysis>,
}

/// Ranking weight of a plugin completion item before its `priority` hint.
/// Keywords sit at 100 and schema objects at 450, so `100 + priority`
/// (priority is clamped to 300) can never outrank a column or a relation.
const PLUGIN_COMPLETION_WEIGHT: i32 = 100;

pub struct IntelligenceService {
    event_bus: Arc<EventBus>,
    schema: Arc<SchemaService>,
    engine: Arc<SemanticEngine>,
    mirrors: Mutex<HashMap<SqlFileId, Buffer>>,
    analyses: RwLock<HashMap<SqlFileId, StoredAnalysis>>,
    /// Bumped on every catalog swap. An analysis started before the swap
    /// may reference arena indices into the old catalog, so it is never
    /// stored or published.
    catalog_generation: AtomicU64,
    installed_snapshot: Mutex<Option<SchemaSnapshotId>>,
    active_connection: RwLock<Option<ConnectionId>>,
    handle: Mutex<Option<Handle>>,
    subscriptions: Mutex<Vec<Subscription>>,
    completion_providers: RwLock<Vec<Arc<dyn CompletionProvider>>>,
}

impl IntelligenceService {
    pub fn new(event_bus: Arc<EventBus>, schema: Arc<SchemaService>) -> Arc<Self> {
        let service = Arc::new(Self {
            event_bus: event_bus.clone(),
            schema,
            engine: Arc::new(SemanticEngine::new(empty_catalog())),
            mirrors: Mutex::new(HashMap::new()),
            analyses: RwLock::new(HashMap::new()),
            catalog_generation: AtomicU64::new(0),
            installed_snapshot: Mutex::new(None),
            active_connection: RwLock::new(None),
            handle: Mutex::new(None),
            subscriptions: Mutex::new(Vec::new()),
            completion_providers: RwLock::new(Vec::new()),
        });

        // Weak handler: the service owns the subscription, so a strong
        // capture would be a reference cycle.
        let weak = Arc::downgrade(&service);
        let subscription = event_bus.subscribe(
            EventFilter::AnyOf(vec![
                AppEventKind::SchemaRefreshed,
                AppEventKind::ConnectionStateChanged,
            ]),
            move |event| {
                if let Some(service) = weak.upgrade() {
                    service.on_event(event);
                }
            },
        );
        service.subscriptions.lock().push(subscription);
        service
    }

    fn handle(&self) -> Option<Handle> {
        self.handle
            .lock()
            .clone()
            .or_else(|| Handle::try_current().ok())
    }

    fn on_event(self: &Arc<Self>, event: &AppEvent) {
        match event {
            AppEvent::SchemaRefreshed {
                connection,
                snapshot,
            } => {
                if let Some(loaded) = self.schema.snapshot(*connection)
                    && loaded.id == *snapshot
                {
                    self.install_snapshot(&loaded);
                }
            }
            AppEvent::ConnectionStateChanged {
                id,
                state: ConnectionState::Connected,
            } => {
                *self.active_connection.write() = Some(*id);
                let Some(handle) = self.handle() else {
                    tracing::warn!("intelligence: no tokio handle; skipping initial refresh");
                    return;
                };
                let weak: Weak<Self> = Arc::downgrade(self);
                let id = *id;
                handle.spawn(async move {
                    if let Some(service) = weak.upgrade() {
                        service.on_connected(id).await;
                    }
                });
            }
            // The server was unreachable on startup (or dropped). The cached
            // catalog is all the intelligence features need, so install it
            // here too: offline startup must not depend on a `Connected` event
            // that will never arrive.
            AppEvent::ConnectionStateChanged {
                id,
                state: ConnectionState::Failed | ConnectionState::Disconnected,
            } => {
                *self.active_connection.write() = Some(*id);
                let Some(handle) = self.handle() else {
                    return;
                };
                let weak: Weak<Self> = Arc::downgrade(self);
                let id = *id;
                handle.spawn(async move {
                    if let Some(service) = weak.upgrade() {
                        service.on_unreachable(id).await;
                    }
                });
            }
            _ => {}
        }
    }

    async fn on_connected(&self, connection: ConnectionId) {
        if let Some(snapshot) = self.schema.load_cached(connection).await {
            self.install_snapshot(&snapshot);
        }
        match self.schema.refresh_incremental(connection).await {
            Ok((snapshot, path)) => {
                self.install_snapshot(&snapshot);
                tracing::info!(?path, "intelligence: schema refreshed");
            }
            Err(e) => {
                tracing::warn!(error = %e, "intelligence: schema refresh failed");
                self.event_bus.publish(AppEvent::SchemaRefreshFailed {
                    connection,
                    message: e.to_string(),
                });
            }
        }
    }

    /// No live connection: serve the persisted catalog so completion, hover
    /// and diagnostics keep working without I/O (docs/12, exit criterion 8).
    async fn on_unreachable(&self, connection: ConnectionId) {
        match self.schema.load_cached(connection).await {
            Some(snapshot) => {
                tracing::info!(
                    connection = %connection,
                    objects = snapshot.objects.len(),
                    "intelligence: offline, using cached catalog"
                );
                self.install_snapshot(&snapshot);
            }
            None => {
                tracing::info!("intelligence: offline, no cached catalog for this connection");
            }
        }
    }

    pub fn install_snapshot(&self, snapshot: &SchemaSnapshot) {
        let mut installed = self.installed_snapshot.lock();
        if installed.as_ref() == Some(&snapshot.id) {
            // The connection bootstrap and its `SchemaRefreshed` event both
            // carry this snapshot; keep the analyses built against it.
            return;
        }
        let catalog = Arc::new(CatalogCache::from_snapshot(snapshot));
        self.replace_catalog(catalog);
        *installed = Some(snapshot.id);
    }

    pub fn set_catalog(&self, catalog: Arc<CatalogCache>) {
        *self.installed_snapshot.lock() = None;
        self.replace_catalog(catalog);
    }

    fn replace_catalog(&self, catalog: Arc<CatalogCache>) {
        // Swap the engine's catalog first, then bump the generation: an
        // analysis that loaded the old generation is guaranteed to see a
        // mismatch after this, and one that loads the new generation already
        // reads the new catalog.
        self.engine.set_catalog(catalog);
        self.catalog_generation.fetch_add(1, Ordering::AcqRel);
        self.analyses.write().clear();
    }

    /// Attach the providers registered by active plugins. Called once at
    /// startup, after `PluginService::activate_all`; a later call replaces the
    /// set (stage 2's theme/panel work activates plugins the same way).
    pub fn set_completion_providers(&self, providers: Vec<Arc<dyn CompletionProvider>>) {
        *self.completion_providers.write() = providers;
    }

    /// Synchronous completion for the UI thread (spec decision 7).
    ///
    /// The engine's ranked list comes first; plugin providers are then merged
    /// into it with the same score formula, so one ordering rule decides the
    /// whole popup (docs/12-sql-intelligence.md).
    pub fn complete(
        &self,
        buffer: &mut Buffer,
        offset: usize,
        explicit: bool,
    ) -> Vec<CompletionItem> {
        let file = buffer.file_id();
        let mut items = self.engine.complete(CompletionRequest {
            file,
            buffer,
            offset,
            explicit,
        });
        self.merge_plugin_completions(&mut items, buffer, file, offset, explicit);
        items
    }

    /// Fan out to plugin providers and fold their candidates into `items`.
    ///
    /// Nothing is copied when no provider is registered — the common case
    /// until a plugin ships one. With providers, the buffer text is handed
    /// over as an owned `String`, because a plugin's request is text-based by
    /// design (`PluginCompletionRequest`); that copy is the price of not
    /// exposing the rope or the analysis model.
    fn merge_plugin_completions(
        &self,
        items: &mut Vec<CompletionItem>,
        buffer: &Buffer,
        file: SqlFileId,
        offset: usize,
        explicit: bool,
    ) {
        let providers = self.completion_providers.read().clone();
        if providers.is_empty() {
            return;
        }

        let text = buffer.text().to_string();
        let prefix = prefix_before(&text, offset);
        let folded_prefix = prefix.to_lowercase();
        let connection = *self.active_connection.read();
        let request = PluginCompletionRequest {
            file,
            text: &text,
            offset,
            prefix,
            explicit,
            connection,
        };

        let started = Instant::now();
        let mut contributed = Vec::new();
        for provider in &providers {
            let provider_started = Instant::now();
            let produced = provider.complete(&request);
            let elapsed = provider_started.elapsed();
            if elapsed > Duration::from_millis(5) {
                tracing::error!(
                    provider = provider.provider_name(),
                    micros = elapsed.as_micros(),
                    "plugin completion provider exceeds the 5 ms budget"
                );
            } else if elapsed > Duration::from_millis(1) {
                tracing::warn!(
                    provider = provider.provider_name(),
                    micros = elapsed.as_micros(),
                    "plugin completion provider is slow"
                );
            }
            contributed.extend(produced);
        }

        for mut item in contributed {
            let Some(quality) = match_quality(prefix, &folded_prefix, &item.label) else {
                continue;
            };
            let weight = PLUGIN_COMPLETION_WEIGHT + item.score.clamp(0, PLUGIN_PRIORITY_MAX);
            item.score = weight * 1000 + quality * 4 - item.label.chars().count() as i32;

            match items
                .iter_mut()
                .find(|existing| existing.label == item.label && existing.kind == item.kind)
            {
                Some(existing) if item.score > existing.score => *existing = item,
                Some(_) => {}
                None => items.push(item),
            }
        }

        items.sort_by(|a, b| {
            b.score
                .cmp(&a.score)
                .then_with(|| a.label.cmp(&b.label))
                .then_with(|| a.kind.cmp(&b.kind))
        });
        items.truncate(COMPLETION_LIMIT);

        let elapsed = started.elapsed();
        if elapsed > Duration::from_millis(5) {
            tracing::error!(
                micros = elapsed.as_micros(),
                providers = providers.len(),
                "plugin completion fan-out exceeded the 5 ms budget"
            );
        }
    }

    /// Synchronous hover for the UI thread.
    pub fn hover(&self, buffer: &mut Buffer, offset: usize) -> Option<HoverInfo> {
        let file = buffer.file_id();
        self.engine.hover(file, buffer, offset)
    }

    /// Analyze an immutable text snapshot on a background task and publish
    /// `DiagnosticsReady`. The revision recorded is the *source* buffer's,
    /// not the mirror's; results older than the stored one, or started before
    /// a catalog swap, are dropped instead of published.
    pub fn analyze(&self, file: SqlFileId, source_revision: u64, text: &str) {
        if let Some(stored) = self.analyses.read().get(&file)
            && stored.source_revision >= source_revision
        {
            return;
        }
        let generation = self.catalog_generation.load(Ordering::Acquire);
        let analysis = {
            let mut mirrors = self.mirrors.lock();
            let buffer = mirrors
                .entry(file)
                .or_insert_with(|| Buffer::new(file, text));
            if let Err(e) = replace_text(buffer, text) {
                tracing::warn!(error = %e, "intelligence: mirror edit failed; rebuilding");
                *buffer = Buffer::new(file, text);
            }
            self.engine.analyze_at(file, source_revision, buffer)
        };

        if self.catalog_generation.load(Ordering::Acquire) != generation {
            tracing::debug!("intelligence: analysis dropped across a catalog swap");
            return;
        }
        let mut analyses = self.analyses.write();
        // Re-check under the lock: `replace_catalog` may have swapped the
        // catalog between the first check and acquiring this write lock.
        if self.catalog_generation.load(Ordering::Acquire) != generation {
            tracing::debug!("intelligence: analysis dropped across a catalog swap");
            return;
        }
        if analyses
            .get(&file)
            .is_some_and(|stored| stored.source_revision >= source_revision)
        {
            return;
        }
        analyses.insert(
            file,
            StoredAnalysis {
                source_revision,
                analysis,
            },
        );
        drop(analyses);
        self.event_bus.publish(AppEvent::DiagnosticsReady { file });
    }

    /// Diagnostics for exactly this text revision, or `None` when the view
    /// has moved on (the analysis is stale and must not be rendered).
    pub fn analysis(&self, file: SqlFileId, source_revision: u64) -> Option<Arc<Analysis>> {
        self.analyses
            .read()
            .get(&file)
            .filter(|stored| stored.source_revision == source_revision)
            .map(|stored| stored.analysis.clone())
    }

    pub fn forget(&self, file: SqlFileId) {
        self.mirrors.lock().remove(&file);
        self.analyses.write().remove(&file);
    }
}

#[async_trait]
impl Service for IntelligenceService {
    fn name(&self) -> &'static str {
        "intelligence"
    }

    async fn start(&self) -> Result<(), ServiceError> {
        *self.handle.lock() = Handle::try_current().ok();
        Ok(())
    }

    async fn stop(&self) -> Result<(), ServiceError> {
        self.subscriptions.lock().clear();
        self.mirrors.lock().clear();
        self.analyses.write().clear();
        *self.handle.lock() = None;
        Ok(())
    }
}

/// Apply `next` to `buffer` as one minimal replacement, so the tree-sitter
/// reparse stays incremental instead of re-parsing the whole document.
fn replace_text(buffer: &mut Buffer, next: &str) -> Result<(), tempr_editor::EditError> {
    let old = buffer.text().to_string();
    if old == next {
        return Ok(());
    }
    let old_bytes = old.as_bytes();
    let new_bytes = next.as_bytes();
    let max = old_bytes.len().min(new_bytes.len());

    let mut prefix = 0;
    while prefix < max && old_bytes[prefix] == new_bytes[prefix] {
        prefix += 1;
    }
    while prefix > 0 && (!old.is_char_boundary(prefix) || !next.is_char_boundary(prefix)) {
        prefix -= 1;
    }

    let max_suffix = max.saturating_sub(prefix);
    let mut suffix = 0;
    while suffix < max_suffix
        && old_bytes[old_bytes.len() - 1 - suffix] == new_bytes[new_bytes.len() - 1 - suffix]
    {
        suffix += 1;
    }
    while suffix > 0
        && (!old.is_char_boundary(old.len() - suffix)
            || !next.is_char_boundary(next.len() - suffix))
    {
        suffix -= 1;
    }

    let old_end = old.len() - suffix;
    let new_end = next.len() - suffix;
    if prefix > old_end || prefix > new_end {
        return Ok(());
    }
    buffer
        .edit(&[(prefix..old_end, &next[prefix..new_end])])
        .map(|_| ())
}

fn empty_catalog() -> Arc<CatalogCache> {
    Arc::new(CatalogCache::from_snapshot(&SchemaSnapshot {
        id: tempr_domain::SchemaSnapshotId::new(),
        connection_id: ConnectionId::new(),
        version: 0,
        fetched_at: chrono::Utc::now(),
        objects: Vec::new(),
        keywords: Vec::new(),
        fingerprints: Vec::new(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex as StdMutex;
    use tempr_domain::{SchemaObject, SchemaObjectId};
    use tempr_intel::CompletionKind;
    use tempr_services_test_catalog::*;

    mod tempr_services_test_catalog {
        use super::*;

        pub fn snapshot() -> SchemaSnapshot {
            let table = SchemaObjectId::new();
            SchemaSnapshot {
                id: tempr_domain::SchemaSnapshotId::new(),
                connection_id: ConnectionId::new(),
                version: 1,
                fetched_at: chrono::Utc::now(),
                objects: vec![
                    SchemaObject::Table {
                        id: table,
                        schema: "public".into(),
                        name: "users".into(),
                        estimated_rows: Some(1),
                    },
                    SchemaObject::Column {
                        id: SchemaObjectId::new(),
                        parent_id: table,
                        name: "id".into(),
                        data_type: "int8".into(),
                        nullable: false,
                        ordinal: 0,
                        default: None,
                    },
                ],
                keywords: vec!["select".into()],
                fingerprints: Vec::new(),
            }
        }
    }

    fn service() -> (Arc<IntelligenceService>, Arc<EventBus>) {
        let bus = Arc::new(EventBus::new());
        let connection = crate::connection::ConnectionService::new(bus.clone());
        let schema = SchemaService::new(bus.clone(), connection);
        let service = IntelligenceService::new(bus.clone(), schema);
        service.set_catalog(Arc::new(CatalogCache::from_snapshot(&snapshot())));
        (service, bus)
    }

    #[tokio::test]
    async fn a_dead_connection_still_installs_the_cached_catalog() {
        // Criterion 8's cold start: the persisted `.tcat` must reach the
        // engine even though no `Connected` event will ever arrive.
        let tmp = tempfile::TempDir::new().expect("temp dir");
        let storage: Arc<dyn tempr_workspace::Storage> =
            Arc::new(tempr_workspace::FileSystemStorage::new(tmp.path()));
        storage.init_workspace_dir().await.expect("init");

        let bus = Arc::new(EventBus::new());
        let connection = ConnectionId::new();
        let schema = SchemaService::with_cache(
            bus.clone(),
            crate::connection::ConnectionService::new(bus.clone()),
            storage.clone(),
        );
        let cached = SchemaSnapshot {
            connection_id: connection,
            ..snapshot()
        };
        storage
            .catalog_cache(connection)
            .save(&cached)
            .await
            .expect("persist catalog");

        let service = IntelligenceService::new(bus, schema);
        service.on_unreachable(connection).await;

        let mut buffer = Buffer::new(SqlFileId::new(), "select * from us");
        let end = buffer.len();
        let items = service.complete(&mut buffer, end, false);
        assert!(
            items.iter().any(|item| item.label == "users"),
            "offline completion must work from the cached catalog"
        );
    }

    #[test]
    fn completion_and_hover_delegate_to_the_engine() {
        let (service, _) = service();
        let mut buffer = Buffer::new(SqlFileId::new(), "select * from us");
        let end = buffer.len();
        let items = service.complete(&mut buffer, end, false);
        assert!(items.iter().any(|item| item.label == "users"));

        let mut buffer = Buffer::new(SqlFileId::new(), "select id from users");
        let hover = service.hover(&mut buffer, 9).expect("hover");
        assert!(
            hover.contents.contains("public.users.id"),
            "{}",
            hover.contents
        );
    }

    #[test]
    fn analyze_publishes_and_stores_only_the_current_revision() {
        let (service, bus) = service();
        let seen = Arc::new(StdMutex::new(Vec::new()));
        let sink = seen.clone();
        let _sub = bus.subscribe(EventFilter::All, move |event| {
            sink.lock()
                .unwrap_or_else(|p| p.into_inner())
                .push(event.kind());
        });

        let file = SqlFileId::new();
        service.analyze(file, 7, "select id from users");
        let analysis = service.analysis(file, 7).expect("stored");
        assert!(analysis.diagnostics.is_empty());
        assert!(
            seen.lock()
                .unwrap_or_else(|p| p.into_inner())
                .contains(&AppEventKind::DiagnosticsReady)
        );
        assert!(service.analysis(file, 6).is_none(), "stale revision");
    }

    #[test]
    fn an_edit_replaces_the_mirror_but_keeps_analyzing() {
        let (service, _) = service();
        let file = SqlFileId::new();
        service.analyze(file, 1, "select id from users");
        service.analyze(file, 2, "select id from users where id = 1");
        let analysis = service.analysis(file, 2).expect("latest");
        assert!(analysis.scopes.iter().any(|s| !s.bindings.is_empty()));
        assert!(service.analysis(file, 1).is_none());
    }

    #[test]
    fn an_out_of_order_analysis_never_overwrites_a_newer_one() {
        let (service, _) = service();
        let file = SqlFileId::new();
        service.analyze(file, 4, "select zz from users");
        assert_eq!(
            service.analysis(file, 4).expect("newer").diagnostics.len(),
            1
        );

        service.analyze(file, 3, "select id from users");
        assert!(service.analysis(file, 3).is_none(), "stale result stored");
        assert_eq!(
            service
                .analysis(file, 4)
                .expect("still current")
                .diagnostics
                .len(),
            1
        );
    }

    #[test]
    fn a_catalog_swap_drops_stored_analyses() {
        let (service, _) = service();
        let file = SqlFileId::new();
        service.analyze(file, 7, "select id from users");
        service.install_snapshot(&snapshot());
        assert!(
            service.analysis(file, 7).is_none(),
            "analyses built against the previous catalog must not survive"
        );
    }

    #[test]
    fn reinstalling_the_same_snapshot_keeps_analyses() {
        let (service, _) = service();
        let cached = snapshot();
        service.install_snapshot(&cached);
        let file = SqlFileId::new();
        service.analyze(file, 9, "select id from users");
        service.install_snapshot(&cached);
        assert!(
            service.analysis(file, 9).is_some(),
            "the same snapshot must be installed idempotently"
        );

        service.install_snapshot(&snapshot());
        assert!(service.analysis(file, 9).is_none());
    }

    #[test]
    fn installation_rebuilds_the_catalog() {
        let (service, bus) = service();
        let schema = snapshot();
        let event = AppEvent::SchemaRefreshed {
            connection: schema.connection_id,
            snapshot: schema.id,
        };
        bus.publish(event);
        let mut buffer = Buffer::new(SqlFileId::new(), "select id from users");
        assert_eq!(
            service
                .complete(&mut buffer, 0, true)
                .first()
                .map(|i| i.kind),
            Some(CompletionKind::Keyword)
        );
    }

    #[tokio::test]
    async fn a_failed_refresh_is_reported_and_keeps_the_catalog() {
        let (service, bus) = service();
        let seen = Arc::new(StdMutex::new(Vec::new()));
        let sink = seen.clone();
        let _sub = bus.subscribe(EventFilter::All, move |event| {
            if let AppEvent::SchemaRefreshFailed { message, .. } = event {
                sink.lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .push(message.clone());
            }
        });
        let connection = ConnectionId::new();
        service.on_connected(connection).await;
        assert!(
            !seen.lock().unwrap_or_else(|p| p.into_inner()).is_empty(),
            "expected a SchemaRefreshFailed event"
        );
        let mut buffer = Buffer::new(SqlFileId::new(), "select id from users");
        let end = buffer.len();
        assert!(service.hover(&mut buffer, end).is_some());
    }

    struct FakeProvider {
        name: &'static str,
        items: Vec<(&'static str, CompletionKind, i32)>,
    }

    impl CompletionProvider for FakeProvider {
        fn provider_name(&self) -> &'static str {
            self.name
        }

        fn complete(&self, req: &PluginCompletionRequest<'_>) -> Vec<CompletionItem> {
            self.items
                .iter()
                .map(|(label, kind, priority)| CompletionItem {
                    label: (*label).to_string(),
                    kind: *kind,
                    detail: String::new(),
                    insert: (*label).to_string(),
                    replace: req.replace_range(),
                    score: *priority,
                })
                .collect()
        }
    }

    fn provider(
        name: &'static str,
        items: Vec<(&'static str, CompletionKind, i32)>,
    ) -> Arc<dyn CompletionProvider> {
        Arc::new(FakeProvider { name, items })
    }

    #[test]
    fn plugin_items_join_the_ranked_list() {
        let (service, _) = service();
        service.set_completion_providers(vec![provider(
            "demo",
            vec![("users_view", CompletionKind::Plugin, 50)],
        )]);
        let mut buffer = Buffer::new(SqlFileId::new(), "select * from us");
        let end = buffer.len();
        let items = service.complete(&mut buffer, end, false);

        assert!(items.iter().any(|item| item.label == "users_view"));
        assert!(items.iter().any(|item| item.label == "users"));
    }

    #[test]
    fn a_provider_item_never_outranks_a_schema_object() {
        let (service, _) = service();
        service.set_completion_providers(vec![provider(
            "demo",
            vec![("users", CompletionKind::Table, PLUGIN_PRIORITY_MAX)],
        )]);
        let mut buffer = Buffer::new(SqlFileId::new(), "select * from us");
        let end = buffer.len();
        let items = service.complete(&mut buffer, end, false);

        let matches: Vec<&CompletionItem> = items.iter().filter(|i| i.label == "users").collect();
        assert_eq!(matches.len(), 1, "deduped against the catalog item");
        assert_eq!(matches[0].kind, CompletionKind::Table);
    }

    #[test]
    fn higher_priority_sorts_first() {
        let (service, _) = service();
        service.set_completion_providers(vec![provider(
            "demo",
            vec![
                ("acme_low", CompletionKind::Plugin, 10),
                ("acme_high", CompletionKind::Plugin, 250),
            ],
        )]);
        let mut buffer = Buffer::new(SqlFileId::new(), "select * from acme");
        let end = buffer.len();
        let items = service.complete(&mut buffer, end, false);

        let low = items
            .iter()
            .position(|i| i.label == "acme_low")
            .expect("low");
        let high = items
            .iter()
            .position(|i| i.label == "acme_high")
            .expect("high");
        assert!(high < low, "priority orders plugin items: {items:?}");
    }

    #[test]
    fn the_merged_list_stays_bounded() {
        let (service, _) = service();
        let labels: Vec<(&'static str, CompletionKind, i32)> = (0..250)
            .map(|i| {
                let label: &'static str = Box::leak(format!("much{i:03}").into_boxed_str());
                (label, CompletionKind::Plugin, 1)
            })
            .collect();
        service.set_completion_providers(vec![provider("demo", labels)]);

        let mut buffer = Buffer::new(SqlFileId::new(), "select * from much");
        let end = buffer.len();
        let items = service.complete(&mut buffer, end, false);
        assert_eq!(items.len(), COMPLETION_LIMIT);
    }

    #[test]
    fn providers_fan_out_without_a_catalog() {
        let bus = Arc::new(EventBus::new());
        let connection = crate::connection::ConnectionService::new(bus.clone());
        let schema = SchemaService::new(bus.clone(), connection);
        let service = IntelligenceService::new(bus, schema);
        service.set_catalog(empty_catalog());
        service.set_completion_providers(vec![provider(
            "demo",
            vec![("offline_snippet", CompletionKind::Plugin, 40)],
        )]);

        let mut buffer = Buffer::new(SqlFileId::new(), "select * from of");
        let end = buffer.len();
        let items = service.complete(&mut buffer, end, false);
        assert!(items.iter().any(|item| item.label == "offline_snippet"));
    }

    #[test]
    fn a_provider_with_no_items_changes_nothing() {
        let (service, _) = service();
        let mut buffer = Buffer::new(SqlFileId::new(), "select * from us");
        let end = buffer.len();
        let before = service.complete(&mut buffer, end, false);

        service.set_completion_providers(vec![provider("silent", Vec::new())]);
        let after = service.complete(&mut buffer, end, false);

        assert_eq!(
            before
                .iter()
                .map(|i| (&i.label, i.kind))
                .collect::<Vec<_>>(),
            after.iter().map(|i| (&i.label, i.kind)).collect::<Vec<_>>()
        );
    }
}
