#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::sync::Arc;

use tempr_domain::{Connection, ConnectionId, DriverKind, SecretRef, TlsMode, Value};
use tempr_events::{AppEventKind, EventBus, EventFilter};
use tempr_services::{ConnectionService, QueryService, SchemaService};

fn pg_connection_string() -> Option<String> {
    std::env::var("DATABASE_URL").ok()
}

fn make_pg_connection(
    id: ConnectionId,
    host: &str,
    port: u16,
    dbname: &str,
    user: &str,
    password: &str,
    tls: TlsMode,
) -> Connection {
    Connection {
        id,
        name: "test-pg".to_string(),
        driver: DriverKind::Postgres,
        host: host.to_string(),
        port,
        database: dbname.to_string(),
        username: user.to_string(),
        password: password.to_string(),
        secret_ref: SecretRef {
            vault_key: "test".to_string(),
        },
        tls,
    }
}

/// Build a `Connection` from a `postgres://` URL, overriding `sslmode`.
fn connection_from_url(raw: &str, tls: TlsMode) -> Connection {
    let url = url::Url::parse(raw).expect("invalid URL");
    make_pg_connection(
        ConnectionId::new(),
        url.host_str().unwrap_or("localhost"),
        url.port().unwrap_or(5432),
        url.path().trim_start_matches('/'),
        url.username(),
        url.password().unwrap_or(""),
        tls,
    )
}

/// `DATABASE_URL_TLS`: a PostgreSQL with `ssl=on` and a self-signed cert
/// (see docs/PROGRESS.md session log for the docker command).
fn pg_tls_connection_string() -> Option<String> {
    std::env::var("DATABASE_URL_TLS").ok()
}

/// Connect with `tls`, return `(services, id)`; panics on failure.
async fn connect_with(
    url: &str,
    tls: TlsMode,
) -> (Arc<EventBus>, Arc<ConnectionService>, ConnectionId) {
    let (bus, cs) = setup_pg_cs();
    let conn = connection_from_url(url, tls);
    cs.connect(&conn).await.expect("connect failed");
    (bus, cs, conn.id)
}

/// `pg_stat_ssl.ssl` for the current backend.
async fn session_is_encrypted(
    bus: Arc<EventBus>,
    cs: Arc<ConnectionService>,
    id: ConnectionId,
) -> bool {
    let qs = QueryService::new(bus, cs);
    let run = qs
        .execute(
            "SELECT ssl FROM pg_stat_ssl WHERE pid = pg_backend_pid()",
            id,
        )
        .await
        .expect("pg_stat_ssl query failed");
    let rs = qs.completed_run(run).unwrap().result_set.unwrap();
    rs.rows[0][0] == Value::Bool(true)
}

fn setup_pg_cs() -> (Arc<EventBus>, Arc<ConnectionService>) {
    let bus = Arc::new(EventBus::new());
    let cs = ConnectionService::new(bus.clone());
    let driver = Arc::new(tempr_db_postgres::PostgresDriver::new());
    cs.register_driver(driver);
    (bus, cs)
}

async fn connect_test_pg(cs: &ConnectionService) -> ConnectionId {
    let conn_str = pg_connection_string().expect("set DATABASE_URL");
    let url = url::Url::parse(&conn_str).expect("invalid DATABASE_URL");

    let id = ConnectionId::new();
    let conn = make_pg_connection(
        id,
        url.host_str().unwrap_or("localhost"),
        url.port().unwrap_or(5432),
        url.path().trim_start_matches('/'),
        url.username(),
        url.password().unwrap_or(""),
        TlsMode::Prefer,
    );

    cs.connect(&conn).await.expect("connect failed");
    id
}

#[tokio::test]
#[ignore = "requires DATABASE_URL env var pointing to a live PostgreSQL instance"]
async fn pg_connect_and_query_select() {
    let (bus, cs) = setup_pg_cs();
    let id = connect_test_pg(&cs).await;

    assert_eq!(cs.state(id), tempr_domain::ConnectionState::Connected);

    let qs = QueryService::new(bus.clone(), cs.clone());
    let run_id = qs
        .execute("SELECT 1 AS num, 'hello' AS greeting", id)
        .await
        .expect("query failed");

    let run = qs.completed_run(run_id).expect("run should be stored");
    assert!(matches!(run.outcome, tempr_domain::QueryOutcome::Success));
    let rs = run.result_set.expect("result set should exist");
    assert_eq!(rs.columns.len(), 2);
    assert_eq!(rs.columns[0].name, "num");
    assert_eq!(rs.columns[1].name, "greeting");
    assert_eq!(rs.rows.len(), 1);
}

#[tokio::test]
#[ignore = "requires DATABASE_URL env var pointing to a live PostgreSQL instance"]
async fn pg_insert_and_select() {
    let (bus, cs) = setup_pg_cs();
    let id = connect_test_pg(&cs).await;

    let qs = QueryService::new(bus.clone(), cs.clone());

    qs.execute("CREATE TEMPORARY TABLE test_tempr (id INT, name TEXT)", id)
        .await
        .expect("create table failed");

    qs.execute("INSERT INTO test_tempr VALUES (1, 'Alice'), (2, 'Bob')", id)
        .await
        .expect("insert failed");

    let run_id = qs
        .execute("SELECT * FROM test_tempr ORDER BY id", id)
        .await
        .expect("select failed");
    let run = qs.completed_run(run_id).expect("run stored");
    let rs = run.result_set.expect("result set");
    assert_eq!(rs.rows.len(), 2);
    assert_eq!(
        rs.rows[0][1],
        tempr_domain::Value::Text("Alice".to_string())
    );
    assert_eq!(rs.rows[1][1], tempr_domain::Value::Text("Bob".to_string()));
}

#[tokio::test]
#[ignore = "requires DATABASE_URL env var pointing to a live PostgreSQL instance"]
async fn pg_decodes_mixed_types() {
    let (bus, cs) = setup_pg_cs();
    let id = connect_test_pg(&cs).await;

    let qs = QueryService::new(bus.clone(), cs.clone());
    let run_id = qs
        .execute(
            "SELECT 42::int4 AS an_int, true AS a_bool, \
             '2025-01-15 10:30:00+00'::timestamptz AS a_timestamp, \
             '550e8400-e29b-41d4-a716-446655440000'::uuid AS a_uuid, \
             3.5::float8 AS a_float, '{\"k\": \"v\"}'::jsonb AS a_json",
            id,
        )
        .await
        .expect("query failed");

    let run = qs.completed_run(run_id).expect("run stored");
    let rs = run.result_set.expect("result set");
    let row = &rs.rows[0];

    assert_eq!(row[0], tempr_domain::Value::Int8(42));
    assert_eq!(row[1], tempr_domain::Value::Bool(true));
    assert!(matches!(row[2], tempr_domain::Value::Timestamp(_)));
    assert!(matches!(row[3], tempr_domain::Value::Uuid(_)));
    assert_eq!(row[4], tempr_domain::Value::Float8(3.5));
    assert!(matches!(row[5], tempr_domain::Value::Json(_)));
}

#[tokio::test]
#[ignore = "requires DATABASE_URL env var pointing to a live PostgreSQL instance"]
async fn pg_insert_returning_id() {
    let (bus, cs) = setup_pg_cs();
    let id = connect_test_pg(&cs).await;

    let qs = QueryService::new(bus.clone(), cs.clone());

    qs.execute(
        "CREATE TEMPORARY TABLE test_tempr_returning (id SERIAL PRIMARY KEY, name TEXT)",
        id,
    )
    .await
    .expect("create table failed");

    let run_id = qs
        .execute(
            "INSERT INTO test_tempr_returning (name) VALUES ('Alice') RETURNING id",
            id,
        )
        .await
        .expect("insert returning failed");

    let run = qs.completed_run(run_id).expect("run stored");
    let rs = run.result_set.expect("RETURNING should yield a result set");
    assert_eq!(rs.rows.len(), 1, "RETURNING should yield the inserted row");
    assert_eq!(rs.rows[0][0], tempr_domain::Value::Int8(1));
}

#[tokio::test]
#[ignore = "requires DATABASE_URL env var pointing to a live PostgreSQL instance"]
async fn pg_streaming_large_result() {
    let (bus, cs) = setup_pg_cs();
    let id = connect_test_pg(&cs).await;

    let qs = QueryService::new(bus.clone(), cs.clone());

    let run_id = qs
        .execute(
            "SELECT generate_series(1, 10000) AS id, md5(random()::text) AS data",
            id,
        )
        .await
        .expect("query failed");

    let run = qs.completed_run(run_id).expect("run stored");
    let rs = run.result_set.expect("result set");
    assert_eq!(rs.total_rows, 10000);
    assert_eq!(rs.columns[0].name, "id");
    assert_eq!(rs.columns[1].name, "data");
}

#[tokio::test]
#[ignore = "requires DATABASE_URL env var pointing to a live PostgreSQL instance"]
async fn pg_auth_failure_returns_error() {
    let bus = Arc::new(EventBus::new());
    let cs = ConnectionService::new(bus.clone());
    let driver = Arc::new(tempr_db_postgres::PostgresDriver::new());
    cs.register_driver(driver);

    let id = ConnectionId::new();
    let conn = Connection {
        id,
        name: "bad-auth".to_string(),
        driver: DriverKind::Postgres,
        host: "localhost".to_string(),
        port: 55432,
        database: "test".to_string(),
        username: "nonexistent_user_abc123".to_string(),
        password: "wrong".to_string(),
        secret_ref: SecretRef {
            vault_key: "test".to_string(),
        },
        tls: TlsMode::Prefer,
    };

    let err = cs
        .connect(&conn)
        .await
        .expect_err("wrong password must fail");
    let msg = err.to_string();
    assert!(
        msg.contains("authentication failed"),
        "auth failure must classify as AuthFailed, got: {msg}"
    );
    assert_eq!(cs.state(id), tempr_domain::ConnectionState::Failed);
}

#[tokio::test]
#[ignore = "requires DATABASE_URL env var pointing to a live PostgreSQL instance"]
async fn pg_schema_refresh() {
    let (bus, cs) = setup_pg_cs();
    let id = connect_test_pg(&cs).await;

    let ss = SchemaService::new(bus.clone(), cs.clone());
    let snapshot = ss.refresh(id).await.expect("schema refresh failed");

    assert!(snapshot.version >= 1);
    assert!(!snapshot.objects.is_empty());

    let snapshot2 = ss.refresh(id).await.expect("second refresh failed");
    assert_eq!(snapshot2.version, snapshot.version + 1);
}

#[tokio::test]
#[ignore = "requires DATABASE_URL env var pointing to a live PostgreSQL instance"]
async fn pg_query_syntax_error() {
    let (bus, cs) = setup_pg_cs();
    let id = connect_test_pg(&cs).await;

    let qs = QueryService::new(bus.clone(), cs.clone());
    let result = qs.execute("SELCT INVALID SYNTAX", id).await;
    assert!(result.is_err());
}

#[tokio::test]
#[ignore = "requires DATABASE_URL env var pointing to a live PostgreSQL instance"]
async fn pg_events_published_during_query() {
    let (bus, cs) = setup_pg_cs();
    let id = connect_test_pg(&cs).await;

    let received: Arc<parking_lot::Mutex<Vec<AppEventKind>>> =
        Arc::new(parking_lot::Mutex::new(Vec::new()));
    let r = received.clone();
    let _sub = bus.subscribe(EventFilter::All, move |event| {
        r.lock().push(event.kind());
    });

    let qs = QueryService::new(bus.clone(), cs.clone());
    let _ = qs.execute("SELECT 1", id).await.expect("query failed");

    let events = received.lock();
    assert!(
        events.contains(&AppEventKind::QueryStarted),
        "expected QueryStarted event"
    );
    assert!(
        events.contains(&AppEventKind::QueryFinished),
        "expected QueryFinished event"
    );
}

/// Streaming path: 100,000 rows arrive in batches through a `RowSink`, with a
/// `RowsReceived` event per batch, and the completed run keeps no rows.
#[tokio::test]
#[ignore = "requires DATABASE_URL env var pointing to a live PostgreSQL instance"]
async fn pg_execute_streaming_100k_rows_in_batches() {
    use tempr_domain::{Batch, ColumnSpec};
    use tempr_services::RowSink;

    #[derive(Default)]
    struct CountingSink {
        columns: parking_lot::Mutex<Vec<ColumnSpec>>,
        batches: parking_lot::Mutex<usize>,
        rows: parking_lot::Mutex<usize>,
        last: parking_lot::Mutex<Option<tempr_domain::Value>>,
    }
    impl RowSink for CountingSink {
        fn columns(&self, columns: &[ColumnSpec]) {
            *self.columns.lock() = columns.to_vec();
        }
        fn batch(&self, batch: Batch) {
            *self.batches.lock() += 1;
            *self.rows.lock() += batch.rows.len();
            if let Some(row) = batch.rows.last() {
                *self.last.lock() = row.first().cloned();
            }
        }
    }

    let (bus, cs) = setup_pg_cs();
    let id = connect_test_pg(&cs).await;
    let qs = QueryService::new(bus.clone(), cs.clone());

    let events: Arc<parking_lot::Mutex<usize>> = Arc::new(parking_lot::Mutex::new(0));
    let e = events.clone();
    let _sub = bus.subscribe(EventFilter::All, move |ev| {
        if ev.kind() == AppEventKind::RowsReceived {
            *e.lock() += 1;
        }
    });

    let sink = Arc::new(CountingSink::default());
    let run_id = qs
        .execute_streaming(
            "SELECT g AS n, 'row ' || g AS label FROM generate_series(1, 100000) g",
            id,
            sink.clone(),
        )
        .await
        .expect("streaming select failed");

    assert_eq!(sink.columns.lock().len(), 2);
    assert_eq!(*sink.rows.lock(), 100_000);
    assert!(*sink.batches.lock() > 1, "expected multiple batches");
    assert_eq!(*events.lock(), *sink.batches.lock());
    assert_eq!(*sink.last.lock(), Some(tempr_domain::Value::Int8(100_000)));

    let run = qs.completed_run(run_id).expect("run stored");
    assert!(
        run.result_set.is_none(),
        "streaming run must not retain rows"
    );
}

// ── TLS (requires DATABASE_URL_TLS → PostgreSQL with ssl=on, self-signed cert) ──

#[tokio::test]
#[ignore = "requires DATABASE_URL_TLS env var pointing to a TLS-enabled PostgreSQL"]
async fn pg_tls_require_encrypts_the_session() {
    let url = pg_tls_connection_string().expect("set DATABASE_URL_TLS");
    let (bus, cs, id) = connect_with(&url, TlsMode::Require).await;
    assert!(
        session_is_encrypted(bus, cs, id).await,
        "sslmode=require must use TLS"
    );
}

#[tokio::test]
#[ignore = "requires DATABASE_URL_TLS env var pointing to a TLS-enabled PostgreSQL"]
async fn pg_tls_prefer_uses_tls_when_the_server_offers_it() {
    let url = pg_tls_connection_string().expect("set DATABASE_URL_TLS");
    let (bus, cs, id) = connect_with(&url, TlsMode::Prefer).await;
    assert!(
        session_is_encrypted(bus, cs, id).await,
        "sslmode=prefer must pick TLS"
    );
}

#[tokio::test]
#[ignore = "requires DATABASE_URL_TLS env var pointing to a TLS-enabled PostgreSQL"]
async fn pg_tls_disable_stays_plaintext_on_a_tls_server() {
    let url = pg_tls_connection_string().expect("set DATABASE_URL_TLS");
    let (bus, cs, id) = connect_with(&url, TlsMode::Disable).await;
    assert!(
        !session_is_encrypted(bus, cs, id).await,
        "sslmode=disable must not use TLS"
    );
}

#[tokio::test]
#[ignore = "requires DATABASE_URL_TLS env var pointing to a TLS-enabled PostgreSQL"]
async fn pg_tls_verify_full_rejects_a_self_signed_certificate() {
    let url = pg_tls_connection_string().expect("set DATABASE_URL_TLS");
    let (_bus, cs) = setup_pg_cs();
    let conn = connection_from_url(&url, TlsMode::VerifyFull);
    let err = cs
        .connect(&conn)
        .await
        .expect_err("self-signed cert must be rejected");
    let msg = err.to_string();
    // rustls names the reason: UnknownIssuer for a plain self-signed leaf,
    // CaUsedAsEndEntity when the self-signed cert carries CA:TRUE.
    assert!(
        msg.contains("invalid peer certificate"),
        "verify-full must fail on the certificate chain, got: {msg}"
    );
    assert_eq!(cs.state(conn.id), tempr_domain::ConnectionState::Failed);
}

#[tokio::test]
#[ignore = "requires DATABASE_URL env var pointing to a live PostgreSQL instance"]
async fn pg_tls_require_fails_against_a_plaintext_only_server() {
    let url = pg_connection_string().expect("set DATABASE_URL");
    // Only meaningful against a server with ssl=off: check first.
    let (bus0, cs0, id0) = connect_with(&url, TlsMode::Disable).await;
    let qs = QueryService::new(bus0, cs0);
    let run = qs.execute("SHOW ssl", id0).await.unwrap();
    let ssl_on = qs.completed_run(run).unwrap().result_set.unwrap().rows[0][0]
        == Value::Text("on".to_string());
    if ssl_on {
        eprintln!("skipping: DATABASE_URL server has ssl=on");
        return;
    }
    let (_bus, cs) = setup_pg_cs();
    let conn = connection_from_url(&url, TlsMode::Require);
    let err = cs
        .connect(&conn)
        .await
        .expect_err("sslmode=require must fail when the server cannot do TLS");
    assert!(
        err.to_string().contains("TLS"),
        "expected a TLS error, got: {err}"
    );
    assert_eq!(cs.state(conn.id), tempr_domain::ConnectionState::Failed);
}
