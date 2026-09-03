//! Tempr binary — wires services + PostgreSQL driver, then hands the main
//! thread to GPUI. All service work runs on the tokio runtime installed by
//! `gpui_tokio`; the UI thread renders and dispatches only.

use anyhow::Result;
use std::sync::Arc;
use tracing::{Level, error, info};
use tracing_subscriber::FmtSubscriber;

use tempr_db::DatabaseDriver;
use tempr_db_postgres::PostgresDriver;
use tempr_events::{EventBus, EventFilter};
use tempr_services::{ConnectionService, QueryService, SchemaService, ServiceRegistry};
use tempr_ui::gpui_compat;

/// Everything the UI needs a handle to. Built before GPUI starts.
///
/// Core services are plain `Arc`s for now; they do not yet implement the
/// `Service` lifecycle trait, so the registry only carries lifecycle-aware
/// services (none in this shell). Tracked in docs/TODO.md.
struct AppServices {
    bus: Arc<EventBus>,
    registry: Arc<ServiceRegistry>,
    _connection: Arc<ConnectionService>,
    _query: Arc<QueryService>,
    _schema: Arc<SchemaService>,
}

fn build_services() -> AppServices {
    let bus = Arc::new(EventBus::new());
    let registry = ServiceRegistry::new();

    let connection = ConnectionService::new(bus.clone());
    let query = QueryService::new(bus.clone(), connection.clone());
    let schema = SchemaService::new(bus.clone(), connection.clone());

    let pg_driver = Arc::new(PostgresDriver::new()) as Arc<dyn DatabaseDriver>;
    connection.register_driver(pg_driver);

    AppServices {
        bus,
        registry,
        _connection: connection,
        _query: query,
        _schema: schema,
    }
}

fn main() -> Result<()> {
    let subscriber = FmtSubscriber::builder()
        .with_max_level(Level::INFO)
        .finish();
    tracing::subscriber::set_global_default(subscriber)?;
    info!("Tempr starting");

    let services = build_services();
    let _event_log = services.bus.subscribe(EventFilter::All, |event| {
        info!(event = ?event.kind(), "event");
    });

    gpui_compat::run_app(move |cx| {
        // Start services on the tokio runtime; the UI thread never blocks.
        let registry = services.registry.clone();
        gpui_compat::spawn_tokio(cx, async move {
            if let Err(e) = registry.start_all().await {
                error!(error = %e, "service startup failed");
            } else {
                info!("all services started");
            }
        })
        .detach();

        if let Err(e) = gpui_compat::open_main_window(cx, "Tempr", |_cx| {
            tempr_ui::MainWindow::new("phase1-demo")
        }) {
            error!(error = %e, "failed to open main window");
            cx.quit();
        }
    });

    Ok(())
}
