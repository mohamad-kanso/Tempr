#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::collections::HashSet;
use std::sync::Arc;

use tempr_db::{ObjectKind, SchemaFingerprint, SchemaScope, SchemaSnapshotEntry};
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

/// Look up a `SchemaSnapshotEntry::Index`'s `native_id` by name via a fresh
/// snapshot.
async fn index_native_id(cs: &ConnectionService, id: ConnectionId, index_name: &str) -> u64 {
    let entries = cs
        .with_metadata_connection_fn(id, |mut conn| async move {
            conn.snapshot_schema(SchemaScope::SearchPath).await
        })
        .await
        .expect("snapshot");
    entries
        .iter()
        .find_map(|e| match e {
            SchemaSnapshotEntry::Index {
                native_id, name, ..
            } if name == index_name => Some(*native_id),
            _ => None,
        })
        .unwrap_or_else(|| panic!("index {index_name} missing from snapshot"))
}

/// Look up a `SchemaSnapshotEntry::Function`'s `native_id` by name via a
/// fresh snapshot.
async fn function_native_id(cs: &ConnectionService, id: ConnectionId, fn_name: &str) -> u64 {
    let entries = cs
        .with_metadata_connection_fn(id, |mut conn| async move {
            conn.snapshot_schema(SchemaScope::SearchPath).await
        })
        .await
        .expect("snapshot");
    entries
        .iter()
        .find_map(|e| match e {
            SchemaSnapshotEntry::Function {
                native_id, name, ..
            } if name == fn_name => Some(*native_id),
            _ => None,
        })
        .unwrap_or_else(|| panic!("function {fn_name} missing from snapshot"))
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

    let qs = QueryService::new(bus.clone(), cs.clone());
    qs.execute("DROP TABLE IF EXISTS schema_refresh_probe", id)
        .await
        .expect("drop probe table failed");
    qs.execute("CREATE TABLE schema_refresh_probe (id int, label text)", id)
        .await
        .expect("create probe table failed");

    let ss = SchemaService::new(bus.clone(), cs.clone());
    let snapshot = ss.refresh(id).await.expect("schema refresh failed");

    assert!(snapshot.version >= 1);

    let find_table = |snap: &tempr_domain::SchemaSnapshot| {
        snap.objects
            .iter()
            .find(|o| matches!(o, tempr_domain::SchemaObject::Table { name, .. } if name == "schema_refresh_probe"))
            .expect("probe table not found in snapshot")
            .clone()
    };
    let table = find_table(&snapshot);
    let table_id = table.id();

    let find_column = |snap: &tempr_domain::SchemaSnapshot, col_name: &str| {
        snap.objects
            .iter()
            .find(|o| {
                matches!(o, tempr_domain::SchemaObject::Column { parent_id, name, .. } if *parent_id == table_id && name == col_name)
            })
            .unwrap_or_else(|| panic!("probe column {col_name} not found in snapshot"))
            .clone()
    };
    find_column(&snapshot, "id");
    find_column(&snapshot, "label");

    let snapshot2 = ss.refresh(id).await.expect("second refresh failed");
    assert_eq!(snapshot2.version, snapshot.version + 1);

    let table2 = find_table(&snapshot2);
    assert_eq!(
        table2.id(),
        table_id,
        "probe table must keep the same id across refreshes"
    );

    qs.execute("DROP TABLE schema_refresh_probe", id)
        .await
        .expect("cleanup drop failed");
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

#[tokio::test]
#[ignore = "requires DATABASE_URL env var pointing to a live PostgreSQL instance"]
async fn pg_snapshot_entries_carry_stable_native_ids() {
    let (_bus, cs) = setup_pg_cs();
    let id = connect_test_pg(&cs).await;

    cs.with_metadata_connection_fn(id, |mut conn| async move {
        conn.execute("DROP TABLE IF EXISTS native_id_probe", &[])
            .await?;
        conn.execute(
            "CREATE TABLE native_id_probe (id bigint primary key, label text not null)",
            &[],
        )
        .await
    })
    .await
    .expect("setup table");

    let first = cs
        .with_metadata_connection_fn(id, |mut conn| async move {
            conn.snapshot_schema(SchemaScope::All).await
        })
        .await
        .expect("first snapshot");
    let second = cs
        .with_metadata_connection_fn(id, |mut conn| async move {
            conn.snapshot_schema(SchemaScope::All).await
        })
        .await
        .expect("second snapshot");

    let table_id = |entries: &[SchemaSnapshotEntry]| -> u64 {
        entries
            .iter()
            .find_map(|e| match e {
                SchemaSnapshotEntry::Table {
                    native_id, name, ..
                } if name == "native_id_probe" => Some(*native_id),
                _ => None,
            })
            .expect("probe table missing from snapshot")
    };
    let a = table_id(&first);
    let b = table_id(&second);
    assert_ne!(a, 0, "native_id must be a real OID");
    assert_eq!(a, b, "native_id must be stable across refreshes");

    // Columns encode (attrelid << 16 | attnum) — same relation, distinct ids.
    let mut col_ids: Vec<u64> = first
        .iter()
        .filter_map(|e| match e {
            SchemaSnapshotEntry::Column {
                native_id,
                parent_table,
                ..
            } if parent_table == "native_id_probe" => Some(*native_id),
            _ => None,
        })
        .collect();
    col_ids.sort_unstable();
    assert_eq!(col_ids.len(), 2, "expected two columns on the probe table");
    assert_ne!(col_ids[0], col_ids[1]);
    assert_eq!(col_ids[0] >> 16, a, "column ids embed their relation OID");

    cs.with_metadata_connection_fn(id, |mut conn| async move {
        conn.execute("DROP TABLE native_id_probe", &[]).await
    })
    .await
    .expect("cleanup");
}

#[tokio::test]
#[ignore = "requires DATABASE_URL env var pointing to a live PostgreSQL instance"]
async fn pg_search_path_scope_excludes_off_path_schemas() {
    let (_bus, cs) = setup_pg_cs();
    let id = connect_test_pg(&cs).await;

    cs.with_metadata_connection_fn(id, |mut conn| async move {
        conn.execute("DROP SCHEMA IF EXISTS off_path CASCADE", &[])
            .await?;
        conn.execute("CREATE SCHEMA off_path", &[]).await?;
        conn.execute("CREATE TABLE off_path.hidden (id int)", &[])
            .await?;
        conn.execute("DROP SCHEMA IF EXISTS on_path CASCADE", &[])
            .await?;
        conn.execute("CREATE SCHEMA on_path", &[]).await?;
        conn.execute("CREATE TABLE on_path.probe (id int)", &[])
            .await?;
        conn.execute("DROP TABLE IF EXISTS on_path_probe", &[])
            .await?;
        conn.execute("CREATE TABLE on_path_probe (id int)", &[])
            .await
    })
    .await
    .expect("setup schemas");

    // `SET search_path` and the snapshot must run in the SAME session: the
    // pooled metadata connection is shared, so a `SET` in one
    // `with_metadata_connection_fn` call is not guaranteed visible to the
    // next. Putting `on_path` on the search_path here — and nowhere else —
    // is what exercises the `current_schemas(false)` half of the clause;
    // `public` alone would pass even if that branch were deleted.
    let scoped = cs
        .with_metadata_connection_fn(id, |mut conn| async move {
            conn.execute("SET search_path TO on_path, public", &[])
                .await?;
            conn.snapshot_schema(SchemaScope::SearchPath).await
        })
        .await
        .expect("search-path snapshot");
    let all = cs
        .with_metadata_connection_fn(id, |mut conn| async move {
            conn.snapshot_schema(SchemaScope::All).await
        })
        .await
        .expect("all snapshot");

    let has = |entries: &[SchemaSnapshotEntry], want_schema: &str, want_name: &str| {
        entries.iter().any(|e| match e {
            SchemaSnapshotEntry::Table { schema, name, .. } => {
                schema == want_schema && name == want_name
            }
            _ => false,
        })
    };

    assert!(
        has(&scoped, "on_path", "probe"),
        "schema on the search_path must be in scope"
    );
    assert!(
        has(&scoped, "public", "on_path_probe"),
        "public must be in scope"
    );
    assert!(
        !has(&scoped, "off_path", "hidden"),
        "off-path schema must be excluded"
    );
    assert!(
        has(&all, "off_path", "hidden"),
        "All scope must still see it"
    );

    cs.with_metadata_connection_fn(id, |mut conn| async move {
        conn.execute("SET search_path TO \"$user\", public", &[])
            .await?;
        conn.execute("DROP SCHEMA off_path CASCADE", &[]).await?;
        conn.execute("DROP SCHEMA on_path CASCADE", &[]).await?;
        conn.execute("DROP TABLE on_path_probe", &[]).await
    })
    .await
    .expect("cleanup");
}

#[tokio::test]
#[ignore = "requires DATABASE_URL env var pointing to a live PostgreSQL instance"]
async fn pg_snapshot_includes_functions() {
    let (_bus, cs) = setup_pg_cs();
    let id = connect_test_pg(&cs).await;

    cs.with_metadata_connection_fn(id, |mut conn| async move {
        conn.execute("DROP FUNCTION IF EXISTS add_two(integer, integer)", &[])
            .await?;
        conn.execute(
            "CREATE FUNCTION add_two(a integer, b integer) RETURNS integer \
             LANGUAGE sql AS $$ SELECT a + b $$",
            &[],
        )
        .await
    })
    .await
    .expect("setup function");

    cs.with_metadata_connection_fn(id, |mut conn| async move {
        conn.execute(
            "DROP FUNCTION IF EXISTS out_param_probe(integer, text)",
            &[],
        )
        .await?;
        conn.execute(
            "CREATE FUNCTION out_param_probe(IN a integer, OUT b integer, IN c text) \
             AS $$ SELECT 1 $$ LANGUAGE sql",
            &[],
        )
        .await
    })
    .await
    .expect("setup out_param_probe function");

    let entries = cs
        .with_metadata_connection_fn(id, |mut conn| async move {
            conn.snapshot_schema(SchemaScope::SearchPath).await
        })
        .await
        .expect("snapshot");

    let func = entries
        .iter()
        .find_map(|e| match e {
            SchemaSnapshotEntry::Function { name, .. } if name == "add_two" => Some(e.clone()),
            _ => None,
        })
        .expect("add_two missing from snapshot");

    match func {
        SchemaSnapshotEntry::Function {
            native_id,
            schema,
            parameters,
            return_type,
            language,
            ..
        } => {
            assert_ne!(native_id, 0);
            assert_eq!(schema, "public");
            assert_eq!(
                parameters,
                vec![
                    ("a".to_string(), "integer".to_string()),
                    ("b".to_string(), "integer".to_string()),
                ]
            );
            assert_eq!(return_type, "integer");
            assert_eq!(language, "sql");
        }
        other => panic!("expected a Function entry, got {other:?}"),
    }

    let out_probe = entries
        .iter()
        .find_map(|e| match e {
            SchemaSnapshotEntry::Function { name, .. } if name == "out_param_probe" => {
                Some(e.clone())
            }
            _ => None,
        })
        .expect("out_param_probe missing from snapshot");

    match out_probe {
        SchemaSnapshotEntry::Function { parameters, .. } => {
            assert_eq!(
                parameters,
                vec![
                    ("a".to_string(), "integer".to_string()),
                    ("c".to_string(), "text".to_string()),
                ]
            );
        }
        other => panic!("expected a Function entry, got {other:?}"),
    }

    cs.with_metadata_connection_fn(id, |mut conn| async move {
        conn.execute("DROP FUNCTION add_two(integer, integer)", &[])
            .await
    })
    .await
    .expect("cleanup");

    cs.with_metadata_connection_fn(id, |mut conn| async move {
        conn.execute("DROP FUNCTION out_param_probe(integer, text)", &[])
            .await
    })
    .await
    .expect("cleanup out_param_probe");
}

#[tokio::test]
#[ignore = "requires DATABASE_URL env var pointing to a live PostgreSQL instance"]
async fn pg_fingerprints_move_only_for_changed_relations() {
    let (_bus, cs) = setup_pg_cs();
    let id = connect_test_pg(&cs).await;

    cs.with_metadata_connection_fn(id, |mut conn| async move {
        conn.execute("DROP TABLE IF EXISTS fp_touched", &[]).await?;
        conn.execute("DROP TABLE IF EXISTS fp_untouched", &[])
            .await?;
        conn.execute("CREATE TABLE fp_touched (id int)", &[])
            .await?;
        conn.execute("CREATE TABLE fp_untouched (id int)", &[])
            .await
    })
    .await
    .expect("setup tables");

    let before = cs
        .with_metadata_connection_fn(id, |mut conn| async move {
            conn.schema_fingerprints(SchemaScope::SearchPath).await
        })
        .await
        .expect("first fingerprints");

    cs.with_metadata_connection_fn(id, |mut conn| async move {
        conn.execute("ALTER TABLE fp_touched ADD COLUMN label text", &[])
            .await
    })
    .await
    .expect("alter table");

    let after = cs
        .with_metadata_connection_fn(id, |mut conn| async move {
            conn.schema_fingerprints(SchemaScope::SearchPath).await
        })
        .await
        .expect("second fingerprints");

    // Map the probe tables to their relation OIDs via a snapshot.
    let entries = cs
        .with_metadata_connection_fn(id, |mut conn| async move {
            conn.snapshot_schema(SchemaScope::SearchPath).await
        })
        .await
        .expect("snapshot");
    let oid_of = |want: &str| -> u64 {
        entries
            .iter()
            .find_map(|e| match e {
                SchemaSnapshotEntry::Table {
                    native_id, name, ..
                } if name == want => Some(*native_id),
                _ => None,
            })
            .unwrap_or_else(|| panic!("{want} missing from snapshot"))
    };
    let version_of = |fps: &[SchemaFingerprint], oid: u64| -> u64 {
        fps.iter()
            .find(|f| f.native_id == oid && f.kind == ObjectKind::Relation)
            .map(|f| f.version)
            .expect("relation fingerprint missing")
    };

    let touched = oid_of("fp_touched");
    let untouched = oid_of("fp_untouched");
    assert!(!before.is_empty(), "fingerprint sweep returned nothing");
    assert_ne!(
        version_of(&before, touched),
        version_of(&after, touched),
        "altered relation must change its fingerprint"
    );
    assert_eq!(
        version_of(&before, untouched),
        version_of(&after, untouched),
        "untouched relation must keep its fingerprint"
    );

    cs.with_metadata_connection_fn(id, |mut conn| async move {
        conn.execute("DROP TABLE fp_touched", &[]).await?;
        conn.execute("DROP TABLE fp_untouched", &[]).await
    })
    .await
    .expect("cleanup");
}

#[tokio::test]
#[ignore = "requires DATABASE_URL env var pointing to a live PostgreSQL instance"]
async fn pg_fingerprints_respect_table_scope_binds() {
    // `SearchPath`/`All` never bind parameters, so they can't catch a bug in
    // the UNION ALL placeholder renumbering. `Table` scope binds two ($1,
    // $2), forcing the second half of the fingerprint query to actually use
    // the renumbered ($3, $4) placeholders — if `renumber_second_clause` or
    // the doubled bind vector were wrong, this scope would either error out
    // (mismatched param count) or silently return the wrong rows.
    let (_bus, cs) = setup_pg_cs();
    let id = connect_test_pg(&cs).await;

    cs.with_metadata_connection_fn(id, |mut conn| async move {
        conn.execute("DROP TABLE IF EXISTS fp_scope_probe", &[])
            .await?;
        conn.execute("DROP TABLE IF EXISTS fp_scope_other", &[])
            .await?;
        conn.execute("CREATE TABLE fp_scope_probe (id int)", &[])
            .await?;
        conn.execute("CREATE TABLE fp_scope_other (id int)", &[])
            .await
    })
    .await
    .expect("setup tables");

    let entries = cs
        .with_metadata_connection_fn(id, |mut conn| async move {
            conn.snapshot_schema(SchemaScope::SearchPath).await
        })
        .await
        .expect("snapshot");
    let oid_of = |want: &str| -> u64 {
        entries
            .iter()
            .find_map(|e| match e {
                SchemaSnapshotEntry::Table {
                    native_id, name, ..
                } if name == want => Some(*native_id),
                _ => None,
            })
            .unwrap_or_else(|| panic!("{want} missing from snapshot"))
    };
    let probe_id = oid_of("fp_scope_probe");
    let other_id = oid_of("fp_scope_other");

    let scoped = cs
        .with_metadata_connection_fn(id, |mut conn| async move {
            conn.schema_fingerprints(SchemaScope::Table {
                schema: "public".to_string(),
                table: "fp_scope_probe".to_string(),
            })
            .await
        })
        .await
        .expect("scoped fingerprints");

    assert!(
        scoped
            .iter()
            .any(|f| f.native_id == probe_id && f.kind == ObjectKind::Relation),
        "table-scoped sweep must include the named table's relation fingerprint"
    );
    assert!(
        scoped
            .iter()
            .any(|f| f.kind == ObjectKind::Column && f.native_id >> 16 == probe_id),
        "table-scoped sweep must include the named table's column fingerprints"
    );
    assert!(
        !scoped.iter().any(|f| {
            (f.kind == ObjectKind::Relation && f.native_id == other_id)
                || (f.kind == ObjectKind::Column && f.native_id >> 16 == other_id)
        }),
        "table-scoped sweep must not leak rows from a different relation"
    );

    cs.with_metadata_connection_fn(id, |mut conn| async move {
        conn.execute("DROP TABLE fp_scope_probe", &[]).await?;
        conn.execute("DROP TABLE fp_scope_other", &[]).await
    })
    .await
    .expect("cleanup");
}

#[tokio::test]
#[ignore = "requires DATABASE_URL env var pointing to a live PostgreSQL instance"]
async fn pg_fingerprints_detect_index_changes() {
    // Before this fix the sweep only queried pg_class (relkind IN ('r','p',
    // 'v','m','f')) and pg_attribute, so an index — relkind 'i' — never
    // appeared in the fingerprint set at all: dropping and recreating one
    // left the parent table's own relation fingerprint byte-identical
    // (`index_update_stats` updates pg_class in place), so the sweep looked
    // clean when it wasn't.
    let (_bus, cs) = setup_pg_cs();
    let id = connect_test_pg(&cs).await;

    cs.with_metadata_connection_fn(id, |mut conn| async move {
        conn.execute("DROP TABLE IF EXISTS fp_idx_probe", &[])
            .await?;
        conn.execute("CREATE TABLE fp_idx_probe (id int, label text)", &[])
            .await?;
        conn.execute("CREATE INDEX fp_idx_probe_a ON fp_idx_probe (id)", &[])
            .await
    })
    .await
    .expect("setup table and index");

    let before = cs
        .with_metadata_connection_fn(id, |mut conn| async move {
            conn.schema_fingerprints(SchemaScope::SearchPath).await
        })
        .await
        .expect("first fingerprints");
    let before_index_oid = index_native_id(&cs, id, "fp_idx_probe_a").await;

    assert!(
        before
            .iter()
            .any(|f| f.kind == ObjectKind::Index && f.native_id == before_index_oid),
        "fingerprint sweep must include the pre-existing index; indexes were \
         previously invisible to the sweep entirely"
    );

    cs.with_metadata_connection_fn(id, |mut conn| async move {
        conn.execute("DROP INDEX fp_idx_probe_a", &[]).await?;
        conn.execute(
            "CREATE UNIQUE INDEX fp_idx_probe_b ON fp_idx_probe (label)",
            &[],
        )
        .await
    })
    .await
    .expect("drop and recreate index");

    let after = cs
        .with_metadata_connection_fn(id, |mut conn| async move {
            conn.schema_fingerprints(SchemaScope::SearchPath).await
        })
        .await
        .expect("second fingerprints");
    let after_index_oid = index_native_id(&cs, id, "fp_idx_probe_b").await;

    let before_indexes: HashSet<u64> = before
        .iter()
        .filter(|f| f.kind == ObjectKind::Index)
        .map(|f| f.native_id)
        .collect();
    let after_indexes: HashSet<u64> = after
        .iter()
        .filter(|f| f.kind == ObjectKind::Index)
        .map(|f| f.native_id)
        .collect();

    assert_ne!(
        before_indexes, after_indexes,
        "dropping and recreating an index must change the set of Index fingerprints"
    );
    assert!(after_indexes.contains(&after_index_oid));
    assert!(!after_indexes.contains(&before_index_oid));

    cs.with_metadata_connection_fn(id, |mut conn| async move {
        conn.execute("DROP TABLE fp_idx_probe", &[]).await
    })
    .await
    .expect("cleanup");
}

#[tokio::test]
#[ignore = "requires DATABASE_URL env var pointing to a live PostgreSQL instance"]
async fn pg_fingerprints_detect_function_changes() {
    // Functions live in pg_proc, which the old sweep never queried, so
    // `CREATE OR REPLACE FUNCTION` with a changed body was invisible.
    let (_bus, cs) = setup_pg_cs();
    let id = connect_test_pg(&cs).await;

    cs.with_metadata_connection_fn(id, |mut conn| async move {
        conn.execute("DROP FUNCTION IF EXISTS fp_fn_probe()", &[])
            .await?;
        conn.execute(
            "CREATE FUNCTION fp_fn_probe() RETURNS integer LANGUAGE sql AS $$ SELECT 1 $$",
            &[],
        )
        .await
    })
    .await
    .expect("setup function");

    let before = cs
        .with_metadata_connection_fn(id, |mut conn| async move {
            conn.schema_fingerprints(SchemaScope::SearchPath).await
        })
        .await
        .expect("first fingerprints");
    let fn_oid = function_native_id(&cs, id, "fp_fn_probe").await;

    let before_version = before
        .iter()
        .find(|f| f.kind == ObjectKind::Function && f.native_id == fn_oid)
        .map(|f| f.version)
        .expect(
            "function fingerprint missing before change; functions were \
             previously invisible to the sweep entirely",
        );

    cs.with_metadata_connection_fn(id, |mut conn| async move {
        conn.execute(
            "CREATE OR REPLACE FUNCTION fp_fn_probe() RETURNS integer \
             LANGUAGE sql AS $$ SELECT 2 $$",
            &[],
        )
        .await
    })
    .await
    .expect("replace function");

    let after = cs
        .with_metadata_connection_fn(id, |mut conn| async move {
            conn.schema_fingerprints(SchemaScope::SearchPath).await
        })
        .await
        .expect("second fingerprints");

    let after_version = after
        .iter()
        .find(|f| f.kind == ObjectKind::Function && f.native_id == fn_oid)
        .map(|f| f.version)
        .expect("function fingerprint missing after change");

    assert_ne!(
        before_version, after_version,
        "CREATE OR REPLACE FUNCTION with a different body must move the function's fingerprint version"
    );

    cs.with_metadata_connection_fn(id, |mut conn| async move {
        conn.execute("DROP FUNCTION fp_fn_probe()", &[]).await
    })
    .await
    .expect("cleanup");
}

#[tokio::test]
#[ignore = "requires DATABASE_URL env var pointing to a live PostgreSQL instance"]
async fn pg_foreign_table_aligns_with_column_query_relkinds() {
    // The table query used to list relkind IN ('r', 'p') while the column
    // query and the fingerprint sweep used ('r', 'p', 'v', 'm', 'f'): a
    // foreign table produced Column entries with no matching Table entry.
    // postgres_fdw lets us create a real foreign table against this same
    // server to exercise that path end to end.
    let (bus, cs) = setup_pg_cs();
    let id = connect_test_pg(&cs).await;

    let qs = QueryService::new(bus.clone(), cs.clone());
    let check_run = qs
        .execute(
            "SELECT 1 FROM pg_extension WHERE extname = 'postgres_fdw'",
            id,
        )
        .await
        .expect("check for postgres_fdw");
    let extension_preexisted = qs
        .completed_run(check_run)
        .and_then(|run| run.result_set)
        .map(|rs| !rs.rows.is_empty())
        .unwrap_or(false);

    cs.with_metadata_connection_fn(id, |mut conn| async move {
        conn.execute("DROP FOREIGN TABLE IF EXISTS fp_ft_probe", &[])
            .await?;
        conn.execute("DROP SERVER IF EXISTS fp_ft_probe_srv CASCADE", &[])
            .await?;
        conn.execute("CREATE EXTENSION IF NOT EXISTS postgres_fdw", &[])
            .await?;
        conn.execute(
            "CREATE SERVER fp_ft_probe_srv FOREIGN DATA WRAPPER postgres_fdw \
             OPTIONS (host 'localhost', dbname 'tempr', port '5432')",
            &[],
        )
        .await?;
        conn.execute(
            "CREATE USER MAPPING FOR CURRENT_USER SERVER fp_ft_probe_srv \
             OPTIONS (user 'tempr', password 'tempr')",
            &[],
        )
        .await?;
        conn.execute(
            "CREATE FOREIGN TABLE fp_ft_probe (id int) SERVER fp_ft_probe_srv \
             OPTIONS (schema_name 'public', table_name 'fp_ft_probe_target')",
            &[],
        )
        .await
    })
    .await
    .expect("setup foreign table");

    let entries = cs
        .with_metadata_connection_fn(id, |mut conn| async move {
            conn.snapshot_schema(SchemaScope::SearchPath).await
        })
        .await
        .expect("snapshot");

    let ft_oid = entries
        .iter()
        .find_map(|e| match e {
            SchemaSnapshotEntry::Table {
                native_id, name, ..
            } if name == "fp_ft_probe" => Some(*native_id),
            _ => None,
        })
        .expect(
            "CREATE FOREIGN TABLE must produce a Table snapshot entry; previously the \
             table query's relkind list omitted 'f', orphaning the foreign table's columns",
        );

    assert!(
        entries.iter().any(|e| matches!(
            e,
            SchemaSnapshotEntry::Column { parent_table, .. } if parent_table == "fp_ft_probe"
        )),
        "the foreign table should still have Column entries"
    );

    let fps = cs
        .with_metadata_connection_fn(id, |mut conn| async move {
            conn.schema_fingerprints(SchemaScope::SearchPath).await
        })
        .await
        .expect("fingerprints");
    assert!(
        fps.iter()
            .any(|f| f.kind == ObjectKind::Relation && f.native_id == ft_oid),
        "the foreign table's relation fingerprint must be present, matching what \
         full introspection returns for the same object"
    );

    cs.with_metadata_connection_fn(id, |mut conn| async move {
        conn.execute("DROP FOREIGN TABLE fp_ft_probe", &[]).await?;
        conn.execute(
            "DROP USER MAPPING FOR CURRENT_USER SERVER fp_ft_probe_srv",
            &[],
        )
        .await?;
        conn.execute("DROP SERVER fp_ft_probe_srv", &[]).await
    })
    .await
    .expect("cleanup foreign table objects");

    if !extension_preexisted {
        cs.with_metadata_connection_fn(id, |mut conn| async move {
            conn.execute("DROP EXTENSION IF EXISTS postgres_fdw", &[])
                .await
        })
        .await
        .expect("cleanup postgres_fdw extension");
    }
}

#[tokio::test]
#[ignore = "requires DATABASE_URL env var pointing to a live PostgreSQL instance"]
async fn pg_keywords_come_from_the_server() {
    let (_bus, cs) = setup_pg_cs();
    let id = connect_test_pg(&cs).await;

    let words = cs
        .with_metadata_connection_fn(id, |mut conn| async move { conn.keywords().await })
        .await
        .expect("keywords");

    assert!(
        words.len() > 100,
        "expected a full keyword list, got {}",
        words.len()
    );
    assert!(
        words.iter().any(|w| w == "select"),
        "keywords are lower-cased"
    );
    assert!(words.iter().any(|w| w == "join"));
}
