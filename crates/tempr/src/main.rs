//! Tempr binary — wires services + PostgreSQL driver, then hands the main
//! thread to GPUI. All service work runs on the tokio runtime installed by
//! `gpui_tokio`; the UI thread renders and dispatches only.

use anyhow::Result;
use std::sync::Arc;
use tracing::{Level, error, info};
use tracing_subscriber::FmtSubscriber;

use tempr_db::DatabaseDriver;
use tempr_db_postgres::PostgresDriver;
use tempr_domain::{Connection, ConnectionId, DriverKind, SecretRef};
use tempr_events::{EventBus, EventFilter};
use tempr_services::{ConnectionService, QueryService, SchemaService, ServiceRegistry};
use tempr_ui::{Services, gpui_compat};

/// Everything the UI needs a handle to. Built before GPUI starts.
///
/// Core services are plain `Arc`s for now; they do not yet implement the
/// `Service` lifecycle trait, so the registry only carries lifecycle-aware
/// services (none in this shell). Tracked in docs/TODO.md.
struct AppServices {
    bus: Arc<EventBus>,
    registry: Arc<ServiceRegistry>,
    connection: Arc<ConnectionService>,
    query: Arc<QueryService>,
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
        connection,
        query,
        _schema: schema,
    }
}

/// Build a `Connection` from `DATABASE_URL` (postgres://user:pass@host:port/db).
/// Phase 1 stand-in for the workspace connection list.
fn connection_from_env() -> Result<Option<Connection>> {
    let Ok(raw) = std::env::var("DATABASE_URL") else {
        return Ok(None);
    };
    let url = url::Url::parse(&raw).map_err(|e| anyhow::anyhow!("DATABASE_URL: {e}"))?;
    if !matches!(url.scheme(), "postgres" | "postgresql") {
        anyhow::bail!("DATABASE_URL: unsupported scheme '{}'", url.scheme());
    }
    // `url` returns userinfo and path percent-encoded; the driver wants the
    // decoded values (e.g. `p%40ss` is the password `p@ss`).
    let decode = |s: &str| {
        percent_encoding::percent_decode_str(s)
            .decode_utf8_lossy()
            .into_owned()
    };
    Ok(Some(Connection {
        id: ConnectionId::new(),
        name: "DATABASE_URL".to_string(),
        driver: DriverKind::Postgres,
        host: url.host_str().unwrap_or("localhost").to_string(),
        port: url.port().unwrap_or(5432),
        database: decode(url.path().trim_start_matches('/')),
        username: decode(url.username()),
        password: decode(url.password().unwrap_or("")),
        secret_ref: SecretRef {
            vault_key: "DATABASE_URL".to_string(),
        },
    }))
}

fn main() -> Result<()> {
    let subscriber = FmtSubscriber::builder()
        .with_max_level(Level::INFO)
        .finish();
    tracing::subscriber::set_global_default(subscriber)?;
    info!("Tempr starting");

    let services = build_services();
    let connection = connection_from_env()?;
    let _event_log = services.bus.subscribe(EventFilter::All, |event| {
        info!(event = ?event.kind(), "event");
    });

    gpui_compat::run_app(move |cx| {
        tempr_ui::bind_keys(cx);
        cx.on_action(|_: &tempr_ui::Quit, cx| cx.quit());

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

        let ui_services = Services {
            bus: services.bus.clone(),
            connection: services.connection.clone(),
            query: services.query.clone(),
        };
        if let Err(e) = gpui_compat::open_main_window(cx, "Tempr", move |window, cx| {
            tempr_ui::MainWindow::new(ui_services, connection, window, cx)
        }) {
            error!(error = %e, "failed to open main window");
            cx.quit();
        }
    });

    Ok(())
}
