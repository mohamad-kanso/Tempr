use anyhow::Result;
use async_trait::async_trait;
use parking_lot::Mutex;
use std::sync::Arc;
use tracing::{Level, info};
use tracing_subscriber::FmtSubscriber;

use tempr_domain::{ConnectionId, ConnectionState, WorkspaceId};
use tempr_events::{AppEvent, AppEventKind, EventBus, EventFilter};
use tempr_services::{Service, ServiceError, ServiceRegistry};
use tempr_workspace::{FileSystemStorage, Storage, WorkspaceManifest};

// ── Demo mock service ──────────────────────────────────────────────────────────

struct MockQueryService {
    event_bus: Arc<EventBus>,
}

impl MockQueryService {
    fn new(event_bus: Arc<EventBus>) -> Arc<Self> {
        Arc::new(Self { event_bus })
    }
}

#[async_trait]
impl Service for MockQueryService {
    fn name(&self) -> &'static str {
        "MockQueryService"
    }

    async fn start(&self) -> Result<(), ServiceError> {
        info!(service = self.name(), "service starting");
        self.event_bus.publish(AppEvent::ConnectionStateChanged {
            id: ConnectionId::new(),
            state: ConnectionState::Connected,
        });
        Ok(())
    }

    async fn stop(&self) -> Result<(), ServiceError> {
        info!(service = self.name(), "service stopping");
        Ok(())
    }
}

// ── Entry point ────────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> Result<()> {
    let subscriber = FmtSubscriber::builder()
        .with_max_level(Level::INFO)
        .finish();
    tracing::subscriber::set_global_default(subscriber)?;

    info!("Tempr Phase 0 demo starting");

    // ── 1. Event bus ──
    let bus = Arc::new(EventBus::new());

    // Subscribe to all events — mirrors what structured logging / a debug panel would do
    let log_sub = {
        let received: Arc<Mutex<Vec<AppEventKind>>> = Arc::new(Mutex::new(Vec::new()));
        let r = received.clone();
        let sub = bus.subscribe(EventFilter::All, move |event| {
            let kind = event.kind();
            info!(event = ?kind, "event bus received");
            r.lock().push(kind);
        });
        (sub, received)
    };
    let (_subscription, received_events) = log_sub;

    // ── 2. Temporary workspace directory ──
    let tmp_dir = tempfile::TempDir::new()?;
    let storage = FileSystemStorage::new(tmp_dir.path());
    storage.init_workspace_dir().await?;

    let manifest = WorkspaceManifest::new("phase0-demo");
    storage.save_manifest(&manifest).await?;
    let loaded = storage.load_manifest().await?;
    info!(workspace = %loaded.workspace.name, "workspace manifest loaded");

    // ── 3. Service registry ──
    let registry = ServiceRegistry::new();
    registry.register(MockQueryService::new(bus.clone()));
    registry.start_all().await?;

    // ── 4. Publish workspace lifecycle events ──
    let ws_id = WorkspaceId::new();
    bus.publish(AppEvent::WorkspaceOpened { id: ws_id });
    bus.publish(AppEvent::WorkspaceClosed { id: ws_id });

    // ── 5. Shutdown ──
    registry.stop_all().await?;

    let events = received_events.lock();
    info!(
        total_events = events.len(),
        ?events,
        "Phase 0 demo complete — event bus delivered all events in order"
    );

    // Verify ordering for the demo
    assert_eq!(events[0], AppEventKind::ConnectionStateChanged); // from mock service start
    assert_eq!(events[1], AppEventKind::WorkspaceOpened);
    assert_eq!(events[2], AppEventKind::WorkspaceClosed);

    info!("Phase 0 exit criteria met — Tempr foundations verified");
    Ok(())
}
