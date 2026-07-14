#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::sync::Arc;

use tempr_domain::{Connection, ConnectionId, DriverKind, SecretRef};
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
    }
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
    };

    let result = cs.connect(&conn).await;
    assert!(result.is_err());
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
