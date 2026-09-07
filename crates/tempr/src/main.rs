//! Tempr binary — wires services + PostgreSQL driver, then hands the main
//! thread to GPUI. All service work runs on the tokio runtime installed by
//! `gpui_tokio`; the UI thread renders and dispatches only.

use anyhow::Result;
use std::path::PathBuf;
use std::sync::Arc;
use tracing::{Level, error, info};
use tracing_subscriber::FmtSubscriber;

use tempr_db::DatabaseDriver;
use tempr_db_postgres::PostgresDriver;
use tempr_domain::{Connection, ConnectionId, DriverKind, SecretRef, TlsMode};
use tempr_events::{EventBus, EventFilter};
use tempr_services::{
    CommandService, ConnectionService, QueryService, SchemaService, ServiceRegistry,
};
use tempr_ui::{DevOptions, Services, gpui_compat};

/// Everything the UI needs a handle to. Built before GPUI starts. The
/// registry owns start/stop ordering: connection → query → schema; `stop_all`
/// runs from the app-quit hook (cancels runs, drains pools).
struct AppServices {
    bus: Arc<EventBus>,
    registry: Arc<ServiceRegistry>,
    connection: Arc<ConnectionService>,
    query: Arc<QueryService>,
    command: Arc<CommandService>,
    _schema: Arc<SchemaService>,
}

fn build_services() -> AppServices {
    let bus = Arc::new(EventBus::new());
    let registry = ServiceRegistry::new();

    let connection = ConnectionService::new(bus.clone());
    let query = QueryService::new(bus.clone(), connection.clone());
    let schema = SchemaService::new(bus.clone(), connection.clone());
    let command = CommandService::new(bus.clone());

    let pg_driver = Arc::new(PostgresDriver::new()) as Arc<dyn DatabaseDriver>;
    connection.register_driver(pg_driver);

    registry.register(connection.clone());
    registry.register(query.clone());
    registry.register(schema.clone());
    registry.register(command.clone());

    AppServices {
        bus,
        registry,
        connection,
        query,
        command,
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
    let tls = match url.query_pairs().find(|(k, _)| k == "sslmode") {
        Some((_, v)) => v
            .parse::<TlsMode>()
            .map_err(|e| anyhow::anyhow!("DATABASE_URL: {e}"))?,
        None => TlsMode::default(),
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
        tls,
    }))
}

/// Path of the `workspace.toml` whose `[keybindings]` form the top layer.
///
/// `TEMPR_WORKSPACE` names either the workspace directory or the manifest
/// file itself; without it the current directory is used. Full workspace open
/// (connection list, recents) is still ahead — see docs/TODO.md.
fn workspace_manifest_path() -> PathBuf {
    match std::env::var_os("TEMPR_WORKSPACE") {
        Some(v) => {
            let p = PathBuf::from(v);
            // A directory (existing, or named without an extension) holds the
            // manifest; anything else is taken as the manifest file itself.
            if p.is_dir() || p.extension().is_none() {
                p.join("workspace.toml")
            } else {
                p
            }
        }
        None => PathBuf::from("workspace.toml"),
    }
}

/// Workspace-level keybinding overrides, plus a notice when the manifest
/// exists but could not be read. A missing manifest is the normal case.
fn workspace_keybindings() -> (tempr_domain::KeybindingOverrides, Option<String>) {
    let path = workspace_manifest_path();
    match tempr_workspace::load_manifest_from(&path) {
        Ok(Some(manifest)) => {
            info!(path = %path.display(), "workspace manifest loaded");
            (manifest.keybindings, None)
        }
        Ok(None) => (Default::default(), None),
        Err(e) => {
            let path = path.display().to_string();
            tracing::warn!(error = %e, path, "ignoring workspace manifest; using defaults");
            (Default::default(), Some(format!("{path} ignored: {e}")))
        }
    }
}

fn main() -> Result<()> {
    let subscriber = FmtSubscriber::builder()
        .with_max_level(Level::INFO)
        .finish();
    tracing::subscriber::set_global_default(subscriber)?;
    info!("Tempr starting");

    let services = build_services();
    let connection = connection_from_env()?;

    // Keybinding layers, lowest first: user settings
    // (~/.config/tempr/settings.toml) then the workspace manifest.
    // A broken settings or manifest file must not prevent the window from
    // opening — both fall back to defaults with a status-bar notice.
    let (user_settings, settings_notice) = match tempr_workspace::load_user_settings() {
        Ok(s) => (s, None),
        Err(e) => {
            let path = tempr_workspace::user_settings_path()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| "settings.toml".into());
            tracing::warn!(error = %e, path, "ignoring user settings; using defaults");
            (
                tempr_workspace::UserSettings::default(),
                Some(format!("{path} ignored: {e}")),
            )
        }
    };
    let (workspace_keys, workspace_notice) = workspace_keybindings();
    services
        .command
        .set_keybinding_layers(vec![user_settings.keybindings.clone(), workspace_keys]);
    // Both files can be broken at once; the status bar shows one line.
    let startup_notice = match (settings_notice, workspace_notice) {
        (Some(a), Some(b)) => Some(format!("{a} · {b}")),
        (a, b) => a.or(b),
    };

    // Keyboard-only audit: print every command with its effective keys.
    if std::env::var("TEMPR_LIST_COMMANDS").is_ok_and(|v| v == "1") {
        for spec in tempr_ui::commands::core_commands() {
            services.command.register(spec.contribution());
        }
        println!(
            "id                                 title                          category     context                pal  keys"
        );
        for c in services.command.commands() {
            println!(
                "{:<34} {:<30} {:<12} {:<22} {:<4} {}",
                c.id.to_string(),
                c.title,
                c.category,
                c.context.as_deref().unwrap_or("(global)"),
                if c.hidden { "-" } else { "yes" },
                c.keystrokes.join(", ")
            );
        }
        return Ok(());
    }
    // Developer knobs (see tempr_ui::DevOptions). TEMPR_STARTUP_SQL runs a
    // statement once connected; TEMPR_BENCH_SCROLL=1 then benchmarks grid
    // scrolling, logs the report and exits.
    let dev = DevOptions {
        startup_sql: std::env::var("TEMPR_STARTUP_SQL")
            .ok()
            .filter(|s| !s.trim().is_empty()),
        bench_scroll_then_exit: std::env::var("TEMPR_BENCH_SCROLL").is_ok_and(|v| v == "1"),
        startup_notice,
    };
    if dev.bench_scroll_then_exit && (dev.startup_sql.is_none() || connection.is_none()) {
        anyhow::bail!("TEMPR_BENCH_SCROLL=1 requires TEMPR_STARTUP_SQL and DATABASE_URL");
    }
    let throttle_inactive = !dev.bench_scroll_then_exit;
    let _event_log = services.bus.subscribe(EventFilter::All, |event| {
        info!(event = ?event.kind(), "event");
    });

    gpui_compat::run_app(move |cx| {
        tempr_ui::commands::install(cx, &services.command);
        cx.on_action(|_: &tempr_ui::Quit, cx| cx.quit());

        // Stop services (cancel runs, drain pools) when the app quits.
        let registry_for_quit = services.registry.clone();
        gpui_compat::on_app_quit(cx, move || {
            let registry = registry_for_quit.clone();
            async move {
                if let Err(e) = registry.stop_all().await {
                    error!(error = %e, "service shutdown failed");
                } else {
                    info!("all services stopped");
                }
            }
        })
        .detach();

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
            command: services.command.clone(),
        };
        if let Err(e) =
            gpui_compat::open_main_window(cx, "Tempr", throttle_inactive, move |window, cx| {
                tempr_ui::MainWindow::new(ui_services, connection, dev, window, cx)
            })
        {
            error!(error = %e, "failed to open main window");
            cx.quit();
        }
    });

    Ok(())
}
