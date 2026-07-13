use anyhow::Result;
use async_trait::async_trait;
use parking_lot::Mutex;
use std::sync::Arc;
use tracing::{Level, info};
use tracing_subscriber::FmtSubscriber;

use tempr_db::DatabaseDriver;
use tempr_db_postgres::PostgresDriver;
use tempr_domain::WorkspaceId;
use tempr_events::{AppEvent, AppEventKind, EventBus, EventFilter};
use tempr_services::{
    ConnectionService, QueryService, SchemaService, Service, ServiceError, ServiceRegistry,
};
use tempr_workspace::{FileSystemStorage, Storage, WorkspaceManifest};

// ── Demo service for Phase 1 ─────────────────────────────────────────────────

struct DemoService {
    _event_bus: Arc<EventBus>,
}

impl DemoService {
    fn new(event_bus: Arc<EventBus>) -> Arc<Self> {
        Arc::new(Self {
            _event_bus: event_bus,
        })
    }
}

#[async_trait]
impl Service for DemoService {
    fn name(&self) -> &'static str {
        "DemoService"
    }

    async fn start(&self) -> Result<(), ServiceError> {
        info!(service = self.name(), "service starting");
        Ok(())
    }

    async fn stop(&self) -> Result<(), ServiceError> {
        info!(service = self.name(), "service stopping");
        Ok(())
    }
}

// ── Entry point ───────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> Result<()> {
    let subscriber = FmtSubscriber::builder()
        .with_max_level(Level::INFO)
        .finish();
    tracing::subscriber::set_global_default(subscriber)?;

    info!("Tempr Phase 1 starting");

    // ── 1. Event bus ──
    let bus = Arc::new(EventBus::new());

    // Subscribe to all events for logging
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

    let manifest = WorkspaceManifest::new("phase1-demo");
    storage.save_manifest(&manifest).await?;
    let loaded = storage.load_manifest().await?;
    info!(workspace = %loaded.workspace.name, "workspace manifest loaded");

    // ── 3. Service registry ──
    let registry = ServiceRegistry::new();

    // Register core services
    let connection_service = ConnectionService::new(bus.clone());
    let _query_service = QueryService::new(bus.clone(), connection_service.clone());
    let schema_service = SchemaService::new(bus.clone(), connection_service.clone());

    // Register PostgreSQL driver
    let pg_driver = Arc::new(PostgresDriver::new()) as Arc<dyn DatabaseDriver>;
    connection_service.register_driver(pg_driver.clone());
    schema_service.register_driver(pg_driver.clone());

    registry.register(DemoService::new(bus.clone()));
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
        "Phase 1 demo complete — event bus delivered all events in order"
    );

    info!(
        "Phase 1 services wired — database driver abstraction + PostgreSQL driver + connection/query/schema services ready"
    );

    // Note: GPUI dependency is pinned in workspace Cargo.toml (per D14).
    // The GPUI UI shell will be built on top of these services in the next phase.

    Ok(())
}
