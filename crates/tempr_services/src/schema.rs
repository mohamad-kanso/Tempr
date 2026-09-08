use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use parking_lot::RwLock;
use tempr_db::{DriverError, ObjectKind, SchemaFingerprint, SchemaScope, SchemaSnapshotEntry};
use tempr_domain::{
    ConnectionId, SchemaFingerprintRecord, SchemaObject, SchemaObjectId, SchemaObjectKind,
    SchemaSnapshot, SchemaSnapshotId,
};
use tempr_events::{AppEvent, EventBus};
use tempr_workspace::Storage;

use crate::connection::ConnectionService;
use crate::{Service, ServiceError};

pub struct SchemaService {
    event_bus: Arc<EventBus>,
    connection_service: Arc<ConnectionService>,
    snapshots: RwLock<HashMap<ConnectionId, Arc<SchemaSnapshot>>>,
    /// Absent when no workspace root is writable: the catalog then lives only
    /// in memory for the session, which is a degradation, never an error.
    storage: Option<Arc<dyn Storage>>,
}

impl SchemaService {
    pub fn new(event_bus: Arc<EventBus>, connection_service: Arc<ConnectionService>) -> Arc<Self> {
        Arc::new(Self {
            event_bus,
            connection_service,
            snapshots: RwLock::new(HashMap::new()),
            storage: None,
        })
    }

    /// Same service, backed by a catalog cache on disk.
    pub fn with_cache(
        event_bus: Arc<EventBus>,
        connection_service: Arc<ConnectionService>,
        storage: Arc<dyn Storage>,
    ) -> Arc<Self> {
        Arc::new(Self {
            event_bus,
            connection_service,
            snapshots: RwLock::new(HashMap::new()),
            storage: Some(storage),
        })
    }

    /// Load this connection's cached snapshot, if any, into memory. Returns
    /// what it loaded so a caller can use the catalog before — or without —
    /// reaching the database.
    pub async fn load_cached(&self, connection_id: ConnectionId) -> Option<Arc<SchemaSnapshot>> {
        let storage = self.storage.as_ref()?;
        let cache = storage.catalog_cache(connection_id);
        match cache.load().await {
            Ok(Some(snapshot)) => {
                let snapshot = Arc::new(snapshot);
                self.snapshots
                    .write()
                    .insert(connection_id, snapshot.clone());
                Some(snapshot)
            }
            Ok(None) => None,
            Err(e) => {
                tracing::warn!(error = %e, "catalog cache load failed; will re-introspect");
                None
            }
        }
    }

    /// Best-effort cache write. Introspection already succeeded, so a failure
    /// here is logged and swallowed — never propagated to the caller.
    async fn save_cached(&self, snapshot: &SchemaSnapshot) {
        let Some(storage) = self.storage.as_ref() else {
            return;
        };
        let cache = storage.catalog_cache(snapshot.connection_id);
        // Skip the write when the content that describes the database is
        // unchanged. Compares only `objects`/`keywords`/`fingerprints` —
        // `id`, `version` and `fetched_at` are fresh on every refresh and
        // would otherwise make this comparison never match.
        if let Ok(Some(existing)) = cache.load().await
            && let (Ok(a), Ok(b)) = (
                tempr_workspace::snapshot_content_hash(&existing),
                tempr_workspace::snapshot_content_hash(snapshot),
            )
            && a == b
        {
            return;
        }
        if let Err(e) = cache.save(snapshot).await {
            tracing::warn!(error = %e, "catalog cache save failed; catalog stays in memory");
        }
    }

    pub async fn refresh(
        &self,
        connection_id: ConnectionId,
    ) -> Result<Arc<SchemaSnapshot>, ServiceError> {
        // One connection, one moment: entries, keywords and fingerprints must
        // describe the same server state or the first diff is already wrong.
        let (entries, keywords, fingerprints) = self
            .connection_service
            .with_metadata_connection_fn(connection_id, |mut conn| async move {
                let entries = conn.snapshot_schema(SchemaScope::SearchPath).await?;
                let keywords = conn.keywords().await?;
                let fingerprints = match conn.schema_fingerprints(SchemaScope::SearchPath).await {
                    Ok(f) => f,
                    // A driver without a sweep simply never refreshes
                    // incrementally; that is not a refresh failure.
                    Err(DriverError::Unsupported(_)) => Vec::new(),
                    Err(e) => return Err(e),
                };
                Ok((entries, keywords, fingerprints))
            })
            .await?;

        let objects = Self::objects_from_entries(connection_id, &entries);
        let fingerprints = fingerprints
            .into_iter()
            .map(|f| SchemaFingerprintRecord {
                kind: domain_kind(f.kind),
                native_id: f.native_id,
                version: f.version,
            })
            .collect();

        let version = self
            .snapshots
            .read()
            .get(&connection_id)
            .map(|s| s.version + 1)
            .unwrap_or(1);

        let snapshot = Arc::new(SchemaSnapshot {
            id: SchemaSnapshotId::new(),
            connection_id,
            version,
            fetched_at: chrono::Utc::now(),
            objects,
            keywords,
            fingerprints,
        });

        self.snapshots
            .write()
            .insert(connection_id, snapshot.clone());
        self.save_cached(&snapshot).await;
        self.event_bus.publish(AppEvent::SchemaRefreshed {
            connection: connection_id,
            snapshot: snapshot.id,
        });
        Ok(snapshot)
    }

    /// Map driver entries to domain objects, deriving every id from
    /// `(connection, kind, native_id)` so the result is reproducible.
    fn objects_from_entries(
        connection_id: ConnectionId,
        entries: &[SchemaSnapshotEntry],
    ) -> Vec<SchemaObject> {
        // First pass: relations, and a (schema, name) → id map for parents.
        let mut parents: HashMap<(String, String), SchemaObjectId> = HashMap::new();
        let mut objects: Vec<SchemaObject> = Vec::new();

        for entry in entries {
            match entry {
                SchemaSnapshotEntry::Table {
                    native_id,
                    schema,
                    name,
                    estimated_rows,
                } => {
                    let id =
                        SchemaObjectId::derived(connection_id, SchemaObjectKind::Table, *native_id);
                    parents.insert((schema.clone(), name.clone()), id);
                    objects.push(SchemaObject::Table {
                        id,
                        schema: schema.clone(),
                        name: name.clone(),
                        estimated_rows: *estimated_rows,
                    });
                }
                SchemaSnapshotEntry::View {
                    native_id,
                    schema,
                    name,
                    definition,
                } => {
                    let id =
                        SchemaObjectId::derived(connection_id, SchemaObjectKind::View, *native_id);
                    parents.insert((schema.clone(), name.clone()), id);
                    objects.push(SchemaObject::View {
                        id,
                        schema: schema.clone(),
                        name: name.clone(),
                        definition: definition.clone(),
                    });
                }
                _ => {}
            }
        }

        // Second pass: children, resolved against the map above.
        for entry in entries {
            match entry {
                SchemaSnapshotEntry::Column {
                    native_id,
                    parent_schema,
                    parent_table,
                    name,
                    data_type,
                    nullable,
                    ordinal,
                    default,
                } => {
                    let Some(parent_id) =
                        parents.get(&(parent_schema.clone(), parent_table.clone()))
                    else {
                        // A column whose relation is out of scope has no
                        // parent to hang from; dropping it is correct, and
                        // inventing a parent id (as this code used to) is not.
                        tracing::debug!(
                            schema = %parent_schema, table = %parent_table, column = %name,
                            "column skipped: parent relation not in scope"
                        );
                        continue;
                    };
                    objects.push(SchemaObject::Column {
                        id: SchemaObjectId::derived(
                            connection_id,
                            SchemaObjectKind::Column,
                            *native_id,
                        ),
                        parent_id: *parent_id,
                        name: name.clone(),
                        data_type: data_type.clone(),
                        nullable: *nullable,
                        ordinal: *ordinal,
                        default: default.clone(),
                    });
                }
                SchemaSnapshotEntry::Index {
                    native_id,
                    parent_schema,
                    parent_table,
                    name,
                    columns,
                    unique,
                    index_type,
                } => {
                    let Some(parent_table_id) =
                        parents.get(&(parent_schema.clone(), parent_table.clone()))
                    else {
                        continue;
                    };
                    objects.push(SchemaObject::Index {
                        id: SchemaObjectId::derived(
                            connection_id,
                            SchemaObjectKind::Index,
                            *native_id,
                        ),
                        parent_table_id: *parent_table_id,
                        name: name.clone(),
                        columns: columns.clone(),
                        unique: *unique,
                        index_type: index_type.clone(),
                    });
                }
                SchemaSnapshotEntry::Function {
                    native_id,
                    schema,
                    name,
                    parameters,
                    return_type,
                    language,
                } => {
                    objects.push(SchemaObject::Function {
                        id: SchemaObjectId::derived(
                            connection_id,
                            SchemaObjectKind::Function,
                            *native_id,
                        ),
                        schema: schema.clone(),
                        name: name.clone(),
                        parameters: parameters.clone(),
                        return_type: return_type.clone(),
                        language: language.clone(),
                    });
                }
                _ => {}
            }
        }
        objects
    }

    pub fn snapshot(&self, connection_id: ConnectionId) -> Option<Arc<SchemaSnapshot>> {
        self.snapshots.read().get(&connection_id).cloned()
    }

    pub fn version(&self, connection_id: ConnectionId) -> Option<u64> {
        self.snapshots.read().get(&connection_id).map(|s| s.version)
    }

    /// Pure diff of a cached fingerprint list against a fresh sweep.
    pub fn diff_fingerprints(
        cached: &[SchemaFingerprintRecord],
        swept: &[SchemaFingerprint],
    ) -> SchemaDelta {
        let mut before: HashMap<(SchemaObjectKind, u64), u64> = cached
            .iter()
            .map(|f| ((f.kind, f.native_id), f.version))
            .collect();

        let mut delta = SchemaDelta::default();
        for f in swept {
            let key = (domain_kind(f.kind), f.native_id);
            match before.remove(&key) {
                Some(version) if version == f.version => {}
                Some(_) => delta.changed.push(key),
                None => delta.added.push(key),
            }
        }
        delta.dropped = before.into_keys().collect();
        // `SchemaObjectKind` has no `Ord` (it is identity data, not a
        // ranking), so order by its stable discriminant instead of the enum
        // itself — sorting only exists here to make the delta's Vec order
        // deterministic for equality assertions.
        let by_kind_then_id = |a: &(SchemaObjectKind, u64), b: &(SchemaObjectKind, u64)| {
            (a.0.discriminant(), a.1).cmp(&(b.0.discriminant(), b.1))
        };
        delta.added.sort_unstable_by(by_kind_then_id);
        delta.changed.sort_unstable_by(by_kind_then_id);
        delta.dropped.sort_unstable_by(by_kind_then_id);
        delta
    }

    /// Sweep fingerprints and re-introspect only the schemas that moved.
    ///
    /// Falls back to a full refresh when there is nothing to diff against (no
    /// cached snapshot, or a cache with no fingerprints), when the driver has
    /// no sweep, when an object appeared that the cache has never seen (its
    /// schema and name are unknown, so nothing can be scoped to it), when
    /// every changed object's schema cannot be resolved from the cache, or
    /// when more than 40% of known objects moved — past that a single full
    /// introspection is the cheaper query.
    pub async fn refresh_incremental(
        &self,
        connection_id: ConnectionId,
    ) -> Result<Arc<SchemaSnapshot>, ServiceError> {
        let cached = match self.snapshot(connection_id) {
            Some(snapshot) => Some(snapshot),
            None => self.load_cached(connection_id).await,
        };
        let Some(cached) = cached.filter(|s| !s.fingerprints.is_empty()) else {
            return self.refresh(connection_id).await;
        };

        let swept = match self
            .connection_service
            .with_metadata_connection_fn(connection_id, |mut conn| async move {
                conn.schema_fingerprints(SchemaScope::SearchPath).await
            })
            .await
        {
            Ok(swept) => swept,
            // `with_metadata_connection_fn` wraps every `DriverError` —
            // including `Unsupported` from a driver with no sweep — into
            // `ServiceError::QueryFailed`; there is no distinct variant to
            // single out "unsupported" from a transient query error, so both
            // fall back here. `NotConnected` covers a pool that dropped
            // between the caller checking in and this call running. Falling
            // back to `refresh` re-runs the same connection lookup, so a
            // `ConnectionNotFound` (missing pool despite a `Connected` state)
            // would fail there identically — propagate it instead of paying
            // for a second doomed call.
            Err(ServiceError::QueryFailed { .. }) | Err(ServiceError::NotConnected { .. }) => {
                return self.refresh(connection_id).await;
            }
            Err(e) => return Err(e),
        };

        let delta = Self::diff_fingerprints(&cached.fingerprints, &swept);
        if delta.is_empty() {
            // Nothing moved: no new snapshot, no event, no cache rewrite.
            return Ok(cached);
        }
        if !delta.added.is_empty() || delta.touched() * 5 > cached.fingerprints.len() * 2 {
            return self.refresh(connection_id).await;
        }

        // Every changed object must resolve to the schema it lives in.
        // `SchemaObject::Table`/`View`/`Function` carry `schema` directly;
        // `Column` and `Index` do not, so their schema is resolved by
        // walking the parent link recorded on the cached object.
        let mut schemas: Vec<String> = delta
            .changed
            .iter()
            .filter_map(|(kind, native_id)| {
                let id = SchemaObjectId::derived(connection_id, *kind, *native_id);
                let object = cached.objects.iter().find(|o| o.id() == id)?;
                object_schema(&cached.objects, object)
            })
            .collect();
        schemas.sort();
        schemas.dedup();
        if schemas.is_empty() {
            // Every changed object was unresolvable (absent from the cached
            // object list, or an orphan with no parent to hang a schema off
            // of) — nothing safe to scope a targeted re-introspection to.
            return self.refresh(connection_id).await;
        }

        let mut entries = Vec::new();
        for schema in &schemas {
            let scope = SchemaScope::Schema(schema.clone());
            let mut part = self
                .connection_service
                .with_metadata_connection_fn(connection_id, |mut conn| async move {
                    conn.snapshot_schema(scope).await
                })
                .await?;
            entries.append(&mut part);
        }
        let refreshed = Self::objects_from_entries(connection_id, &entries);

        // A dropped fingerprint's `kind` came from `domain_kind`, which folds
        // every `pg_class` row into `Table` — a sweep cannot tell a table
        // from a view. So a dropped view's fingerprint says `Table`, and the
        // id derived from it never matches the view's actual cached id
        // (derived under `View`). Derive both candidates for a `Table`-kind
        // drop and let whichever one is actually cached be removed — ids are
        // cheap to over-compute but a stale object that slips past this
        // filter is never removed again. `Column`, `Index`, and `Function`
        // are unambiguous: `domain_kind` maps them 1:1, so no other kind
        // needs this treatment.
        let dropped: HashSet<SchemaObjectId> = delta
            .dropped
            .iter()
            .flat_map(|(kind, native_id)| {
                let mut ids = vec![SchemaObjectId::derived(connection_id, *kind, *native_id)];
                if *kind == SchemaObjectKind::Table {
                    ids.push(SchemaObjectId::derived(
                        connection_id,
                        SchemaObjectKind::View,
                        *native_id,
                    ));
                }
                ids
            })
            .collect();
        let objects = Self::splice_objects(&cached.objects, refreshed, &dropped, &schemas);

        let snapshot = Arc::new(SchemaSnapshot {
            id: SchemaSnapshotId::new(),
            connection_id,
            version: cached.version + 1,
            fetched_at: chrono::Utc::now(),
            objects,
            keywords: cached.keywords.clone(),
            fingerprints: swept
                .into_iter()
                .map(|f| SchemaFingerprintRecord {
                    kind: domain_kind(f.kind),
                    native_id: f.native_id,
                    version: f.version,
                })
                .collect(),
        });

        self.snapshots
            .write()
            .insert(connection_id, snapshot.clone());
        self.save_cached(&snapshot).await;
        self.event_bus.publish(AppEvent::SchemaRefreshed {
            connection: connection_id,
            snapshot: snapshot.id,
        });
        Ok(snapshot)
    }

    /// Combine the surviving cached objects with freshly introspected ones.
    /// Anything in a re-introspected schema is superseded wholesale, and
    /// anything the sweep reported as dropped is removed — including a view
    /// whose fingerprint could only say "some relation".
    fn splice_objects(
        cached: &[SchemaObject],
        refreshed: Vec<SchemaObject>,
        dropped: &HashSet<SchemaObjectId>,
        touched_schemas: &[String],
    ) -> Vec<SchemaObject> {
        let mut objects: Vec<SchemaObject> = cached
            .iter()
            .filter(|o| {
                if dropped.contains(&o.id()) {
                    return false;
                }
                match object_schema(cached, o) {
                    Some(schema) => !touched_schemas.contains(&schema),
                    None => true,
                }
            })
            .cloned()
            .collect();
        objects.extend(refreshed);
        objects
    }
}

/// What a fingerprint sweep says changed since the cached snapshot.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct SchemaDelta {
    pub added: Vec<(SchemaObjectKind, u64)>,
    pub changed: Vec<(SchemaObjectKind, u64)>,
    pub dropped: Vec<(SchemaObjectKind, u64)>,
}

impl SchemaDelta {
    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.changed.is_empty() && self.dropped.is_empty()
    }

    /// Objects touched, weighed against the full-refresh threshold.
    pub fn touched(&self) -> usize {
        self.added.len() + self.changed.len() + self.dropped.len()
    }
}

/// The schema a domain object lives in. `Table`/`View`/`Function` carry it
/// directly; `Column` and `Index` do not, so it is resolved by walking the
/// parent link recorded on the object, one hop, against the same object list.
fn object_schema(objects: &[SchemaObject], object: &SchemaObject) -> Option<String> {
    match object {
        SchemaObject::Table { schema, .. }
        | SchemaObject::View { schema, .. }
        | SchemaObject::Function { schema, .. } => Some(schema.clone()),
        SchemaObject::Column { parent_id, .. } => objects
            .iter()
            .find(|o| o.id() == *parent_id)
            .and_then(|parent| object_schema(objects, parent)),
        SchemaObject::Index {
            parent_table_id, ..
        } => objects
            .iter()
            .find(|o| o.id() == *parent_table_id)
            .and_then(|parent| object_schema(objects, parent)),
    }
}

/// The sweep cannot tell a table from a view — both are `pg_class` rows —
/// while the domain can. Mapping `Relation` to `Table` is safe because a
/// fingerprint key is only ever compared against another fingerprint key.
fn domain_kind(kind: ObjectKind) -> SchemaObjectKind {
    match kind {
        ObjectKind::Relation => SchemaObjectKind::Table,
        ObjectKind::Column => SchemaObjectKind::Column,
        ObjectKind::Index => SchemaObjectKind::Index,
        ObjectKind::Function => SchemaObjectKind::Function,
    }
}

#[async_trait::async_trait]
impl Service for SchemaService {
    fn name(&self) -> &'static str {
        "SchemaService"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempr_domain::ConnectionId;

    fn make_event_bus() -> Arc<EventBus> {
        Arc::new(EventBus::new())
    }

    #[tokio::test]
    async fn snapshot_returns_none_for_unknown() {
        let bus = make_event_bus();
        let cs = ConnectionService::new(bus.clone());
        let svc = SchemaService::new(bus, cs);
        let id = ConnectionId::new();
        assert!(svc.snapshot(id).is_none());
    }

    #[tokio::test]
    async fn version_returns_none_for_unknown() {
        let bus = make_event_bus();
        let cs = ConnectionService::new(bus.clone());
        let svc = SchemaService::new(bus, cs);
        let id = ConnectionId::new();
        assert!(svc.version(id).is_none());
    }

    #[tokio::test]
    async fn refresh_fails_without_connection() {
        let bus = make_event_bus();
        let cs = ConnectionService::new(bus.clone());
        let svc = SchemaService::new(bus, cs);
        let id = ConnectionId::new();
        let result = svc.refresh(id).await;
        assert!(result.is_err());
    }

    fn table_entry(native_id: u64, schema: &str, name: &str) -> SchemaSnapshotEntry {
        SchemaSnapshotEntry::Table {
            native_id,
            schema: schema.to_string(),
            name: name.to_string(),
            estimated_rows: Some(42),
        }
    }

    #[test]
    fn objects_from_entries_derives_stable_ids_and_links_children() {
        let connection = ConnectionId::new();
        let entries = vec![
            table_entry(16384, "public", "users"),
            SchemaSnapshotEntry::View {
                native_id: 16385,
                schema: "public".to_string(),
                name: "active_users".to_string(),
                definition: "SELECT 1".to_string(),
            },
            SchemaSnapshotEntry::Column {
                native_id: (16384u64 << 16) | 1,
                parent_schema: "public".to_string(),
                parent_table: "users".to_string(),
                name: "id".to_string(),
                data_type: "bigint".to_string(),
                nullable: false,
                ordinal: 1,
                default: None,
            },
        ];

        let first = SchemaService::objects_from_entries(connection, &entries);
        let second = SchemaService::objects_from_entries(connection, &entries);
        assert_eq!(
            first.iter().map(|o| o.id()).collect::<Vec<_>>(),
            second.iter().map(|o| o.id()).collect::<Vec<_>>(),
            "mapping the same entries twice must produce the same ids"
        );

        let table_id = SchemaObjectId::derived(connection, SchemaObjectKind::Table, 16384);
        let column_parent = first
            .iter()
            .find_map(|o| match o {
                SchemaObject::Column {
                    parent_id, name, ..
                } if name == "id" => Some(*parent_id),
                _ => None,
            })
            .expect("column missing");
        assert_eq!(
            column_parent, table_id,
            "column must hang off its real table id"
        );

        let rows = first.iter().find_map(|o| match o {
            SchemaObject::Table { estimated_rows, .. } => Some(*estimated_rows),
            _ => None,
        });
        assert_eq!(
            rows,
            Some(Some(42)),
            "estimated_rows must survive the mapping"
        );

        let definition = first.iter().find_map(|o| match o {
            SchemaObject::View { definition, .. } => Some(definition.clone()),
            _ => None,
        });
        assert_eq!(
            definition,
            Some("SELECT 1".to_string()),
            "view definition must survive the mapping"
        );
    }

    #[test]
    fn a_column_whose_parent_is_out_of_scope_is_dropped() {
        // Search-path scoping means a column can arrive without its relation.
        // Inventing a parent id (what this code used to do) produces an orphan
        // that no consumer can resolve; dropping it is correct.
        let connection = ConnectionId::new();
        let entries = vec![SchemaSnapshotEntry::Column {
            native_id: (99999u64 << 16) | 1,
            parent_schema: "other".to_string(),
            parent_table: "unseen".to_string(),
            name: "id".to_string(),
            data_type: "bigint".to_string(),
            nullable: true,
            ordinal: 1,
            default: None,
        }];
        assert!(SchemaService::objects_from_entries(connection, &entries).is_empty());
    }

    #[test]
    fn ids_differ_per_connection() {
        let entries = vec![table_entry(16384, "public", "users")];
        let a = SchemaService::objects_from_entries(ConnectionId::new(), &entries);
        let b = SchemaService::objects_from_entries(ConnectionId::new(), &entries);
        assert_ne!(a[0].id(), b[0].id(), "two databases may share OIDs");
    }

    #[tokio::test]
    async fn save_cached_skips_rewrite_when_content_is_unchanged() {
        // Real FileSystemStorage over a tempdir, not a mock: this exercises
        // save_cached's dedup against actual encode/decode round-trips.
        let tmp = tempfile::TempDir::new().expect("temp dir");
        let storage: Arc<dyn tempr_workspace::Storage> =
            Arc::new(tempr_workspace::FileSystemStorage::new(tmp.path()));
        storage.init_workspace_dir().await.expect("init");

        let bus = make_event_bus();
        let cs = ConnectionService::new(bus.clone());
        let svc = SchemaService::with_cache(bus, cs, storage.clone());

        let connection = ConnectionId::new();
        let base = SchemaSnapshot {
            id: SchemaSnapshotId::new(),
            connection_id: connection,
            version: 1,
            fetched_at: chrono::Utc::now(),
            objects: vec![],
            keywords: vec!["select".to_string()],
            fingerprints: vec![],
        };
        svc.save_cached(&base).await;

        let cache = storage.catalog_cache(connection);
        let first_on_disk = cache.load().await.expect("load").expect("file exists");

        // Same content, fresh bookkeeping — exactly what a no-op refresh
        // produces (new id, bumped version, new fetched_at).
        let mut second = base.clone();
        second.id = SchemaSnapshotId::new();
        second.version = base.version + 1;
        second.fetched_at = base.fetched_at + chrono::Duration::hours(1);
        svc.save_cached(&second).await;

        let after_second = cache.load().await.expect("load").expect("file exists");
        assert_eq!(
            after_second.id, first_on_disk.id,
            "unchanged content must not trigger a rewrite"
        );

        // A real content change must still be written.
        let mut third = second.clone();
        third.keywords.push("merge".to_string());
        svc.save_cached(&third).await;

        let after_third = cache.load().await.expect("load").expect("file exists");
        assert_eq!(
            after_third.id, third.id,
            "a content change must be written to disk"
        );
    }

    #[test]
    fn diff_classifies_changed_new_and_dropped() {
        let cached = vec![
            SchemaFingerprintRecord {
                kind: SchemaObjectKind::Table,
                native_id: 1,
                version: 10,
            },
            SchemaFingerprintRecord {
                kind: SchemaObjectKind::Table,
                native_id: 2,
                version: 20,
            },
            SchemaFingerprintRecord {
                kind: SchemaObjectKind::Column,
                native_id: 100,
                version: 30,
            },
        ];
        let swept = vec![
            SchemaFingerprint {
                native_id: 1,
                kind: ObjectKind::Relation,
                version: 10,
            }, // unchanged
            SchemaFingerprint {
                native_id: 2,
                kind: ObjectKind::Relation,
                version: 21,
            }, // changed
            SchemaFingerprint {
                native_id: 3,
                kind: ObjectKind::Relation,
                version: 40,
            }, // new
               // column 100 absent → dropped
        ];

        let delta = SchemaService::diff_fingerprints(&cached, &swept);
        assert_eq!(delta.changed, vec![(SchemaObjectKind::Table, 2)]);
        assert_eq!(delta.added, vec![(SchemaObjectKind::Table, 3)]);
        assert_eq!(delta.dropped, vec![(SchemaObjectKind::Column, 100)]);
        assert_eq!(delta.touched(), 3);
        assert!(!delta.is_empty());
    }

    #[test]
    fn an_identical_sweep_is_an_empty_delta() {
        let cached = vec![SchemaFingerprintRecord {
            kind: SchemaObjectKind::Table,
            native_id: 1,
            version: 10,
        }];
        let swept = vec![SchemaFingerprint {
            native_id: 1,
            kind: ObjectKind::Relation,
            version: 10,
        }];
        let delta = SchemaService::diff_fingerprints(&cached, &swept);
        assert!(delta.is_empty());
        assert_eq!(delta.touched(), 0);
    }

    #[test]
    fn a_column_id_never_matches_a_relation_id_with_the_same_number() {
        // The pair (kind, native_id) is the key precisely because these two
        // numbers can coincide on a database whose OID counter has wrapped.
        let cached = vec![SchemaFingerprintRecord {
            kind: SchemaObjectKind::Column,
            native_id: 16384,
            version: 1,
        }];
        let swept = vec![SchemaFingerprint {
            native_id: 16384,
            kind: ObjectKind::Relation,
            version: 1,
        }];
        let delta = SchemaService::diff_fingerprints(&cached, &swept);
        assert_eq!(delta.added, vec![(SchemaObjectKind::Table, 16384)]);
        assert_eq!(delta.dropped, vec![(SchemaObjectKind::Column, 16384)]);
    }

    fn test_table(
        connection: ConnectionId,
        native_id: u64,
        schema: &str,
        name: &str,
    ) -> SchemaObject {
        SchemaObject::Table {
            id: SchemaObjectId::derived(connection, SchemaObjectKind::Table, native_id),
            schema: schema.to_string(),
            name: name.to_string(),
            estimated_rows: None,
        }
    }

    fn test_view(
        connection: ConnectionId,
        native_id: u64,
        schema: &str,
        name: &str,
    ) -> SchemaObject {
        SchemaObject::View {
            id: SchemaObjectId::derived(connection, SchemaObjectKind::View, native_id),
            schema: schema.to_string(),
            name: name.to_string(),
            definition: "SELECT 1".to_string(),
        }
    }

    fn test_column(
        connection: ConnectionId,
        native_id: u64,
        parent_id: SchemaObjectId,
        name: &str,
    ) -> SchemaObject {
        SchemaObject::Column {
            id: SchemaObjectId::derived(connection, SchemaObjectKind::Column, native_id),
            parent_id,
            name: name.to_string(),
            data_type: "text".to_string(),
            nullable: true,
            ordinal: 1,
            default: None,
        }
    }

    /// Mirrors the production `dropped` construction in `refresh_incremental`:
    /// a `Table`-kind fingerprint might really be a dropped view (the sweep
    /// cannot tell them apart), so both candidate ids are derived.
    fn dropped_ids(
        connection: ConnectionId,
        drops: &[(SchemaObjectKind, u64)],
    ) -> HashSet<SchemaObjectId> {
        drops
            .iter()
            .flat_map(|(kind, native_id)| {
                let mut ids = vec![SchemaObjectId::derived(connection, *kind, *native_id)];
                if *kind == SchemaObjectKind::Table {
                    ids.push(SchemaObjectId::derived(
                        connection,
                        SchemaObjectKind::View,
                        *native_id,
                    ));
                }
                ids
            })
            .collect()
    }

    #[test]
    fn splice_removes_a_dropped_view_even_though_its_fingerprint_said_table() {
        // The Critical case: `CREATE VIEW public.v` then `DROP VIEW public.v`
        // with nothing else changed in `public`. The sweep's dropped
        // fingerprint can only say `(Table, oid)` — `domain_kind` folds every
        // `pg_class` row into `Table` — while the cached view's real id was
        // derived under `View`. Without deriving both candidates, the view
        // would survive every incremental refresh forever.
        let connection = ConnectionId::new();
        let view = test_view(connection, 500, "public", "v");
        let cached = vec![view];
        let dropped = dropped_ids(connection, &[(SchemaObjectKind::Table, 500)]);

        let objects = SchemaService::splice_objects(&cached, Vec::new(), &dropped, &[]);

        assert!(
            objects.is_empty(),
            "dropped view must not survive: {objects:?}"
        );
    }

    #[test]
    fn splice_keeps_an_untouched_schema_object_despite_a_same_named_touched_one() {
        let connection = ConnectionId::new();
        let untouched = test_table(connection, 1, "public", "users");
        let untouched_id = untouched.id();
        let touched = test_table(connection, 2, "reporting", "users");
        let cached = vec![untouched, touched];
        let touched_schemas = vec!["reporting".to_string()];

        let objects =
            SchemaService::splice_objects(&cached, Vec::new(), &HashSet::new(), &touched_schemas);

        assert_eq!(
            objects.iter().map(|o| o.id()).collect::<Vec<_>>(),
            vec![untouched_id],
            "only the untouched-schema object must survive"
        );
    }

    #[test]
    fn splice_drops_a_touched_schema_object_the_reintrospection_did_not_return() {
        // The table was dropped from a schema that got re-read: the fresh
        // introspection of that schema simply no longer mentions it.
        let connection = ConnectionId::new();
        let gone = test_table(connection, 1, "public", "gone");
        let cached = vec![gone];
        let touched_schemas = vec!["public".to_string()];

        let objects =
            SchemaService::splice_objects(&cached, Vec::new(), &HashSet::new(), &touched_schemas);

        assert!(
            objects.is_empty(),
            "superseded table must be gone: {objects:?}"
        );
    }

    #[test]
    fn splice_does_not_duplicate_a_column_whose_parent_table_was_superseded() {
        let connection = ConnectionId::new();
        let old_table = test_table(connection, 1, "public", "t");
        let old_column = test_column(connection, 10, old_table.id(), "id");
        let cached = vec![old_table, old_column];
        let touched_schemas = vec!["public".to_string()];

        let new_table = test_table(connection, 1, "public", "t");
        let new_table_id = new_table.id();
        let new_column = test_column(connection, 10, new_table.id(), "id");
        let new_column_id = new_column.id();
        let refreshed = vec![new_table, new_column];

        let objects =
            SchemaService::splice_objects(&cached, refreshed, &HashSet::new(), &touched_schemas);

        let ids: HashSet<_> = objects.iter().map(|o| o.id()).collect();
        let expected: HashSet<_> = [new_table_id, new_column_id].into_iter().collect();
        assert_eq!(ids, expected, "only the refreshed copies must remain");
        let column_count = objects
            .iter()
            .filter(|o| matches!(o, SchemaObject::Column { name, .. } if name == "id"))
            .count();
        assert_eq!(column_count, 1, "column must not appear twice");
    }
}
