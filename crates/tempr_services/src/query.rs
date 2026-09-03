use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::RwLock;
use tempr_db::CancelHandle;
use tempr_domain::{
    Batch, ColumnMeta, ColumnSpec, ConnectionId, Query, QueryOutcome, QueryRun, QueryRunId,
    ResultSet, Value,
};
use tempr_events::{AppEvent, EventBus};

use crate::ServiceError;
use crate::connection::ConnectionService;

struct ActiveRun {
    query_run: QueryRun,
    cancel_handle: Option<Box<dyn CancelHandle>>,
    /// Set by `cancel()`; `finish()` then records `Cancelled` instead of the
    /// driver's cancellation error.
    cancelled: bool,
}

/// Receives a query's results as they stream in. Called from the query task
/// (tokio), so implementations must be `Send + Sync` and must not block —
/// hand the data to a channel or a lock-free buffer and return.
pub trait RowSink: Send + Sync {
    /// Column metadata, delivered exactly once before the first batch.
    fn columns(&self, columns: &[ColumnSpec]);
    /// One batch of rows, in arrival order.
    fn batch(&self, batch: Batch);
}

/// Sink used by [`QueryService::execute`] to materialise a full `ResultSet`.
#[derive(Default)]
struct CollectingSink {
    rows: parking_lot::Mutex<Vec<Vec<Value>>>,
}

impl RowSink for CollectingSink {
    fn columns(&self, _columns: &[ColumnSpec]) {}
    fn batch(&self, batch: Batch) {
        self.rows.lock().extend(batch.rows);
    }
}

fn column_meta(columns: &[ColumnSpec]) -> Vec<ColumnMeta> {
    columns
        .iter()
        .map(|c| ColumnMeta {
            name: c.name.clone(),
            data_type: c.data_type.clone(),
            nullable: c.nullable,
            ordinal: c.ordinal,
        })
        .collect()
}

pub struct QueryService {
    event_bus: Arc<EventBus>,
    connection_service: Arc<ConnectionService>,
    active_runs: RwLock<HashMap<QueryRunId, ActiveRun>>,
    completed_runs: RwLock<HashMap<QueryRunId, QueryRun>>,
}

impl QueryService {
    pub fn new(event_bus: Arc<EventBus>, connection_service: Arc<ConnectionService>) -> Arc<Self> {
        Arc::new(Self {
            event_bus,
            connection_service,
            active_runs: RwLock::new(HashMap::new()),
            completed_runs: RwLock::new(HashMap::new()),
        })
    }

    /// Execute `sql`, collecting every row into the completed run's
    /// `ResultSet`. Prefer [`Self::execute_streaming`] for anything a user
    /// will look at while it is still arriving.
    pub async fn execute(
        &self,
        sql: &str,
        connection_id: ConnectionId,
    ) -> Result<QueryRunId, ServiceError> {
        let sink = Arc::new(CollectingSink::default());
        let (run_id, result) = self.run(sql, connection_id, sink.clone()).await;
        let result_set = result.as_ref().ok().map(|(columns, total_rows)| {
            let rows = std::mem::take(&mut *sink.rows.lock());
            ResultSet {
                columns: columns.clone(),
                rows,
                total_rows: *total_rows,
                truncated: false,
            }
        });
        self.finish(run_id, result, result_set)
    }

    /// Execute `sql`, delivering columns and then each batch to `sink` as
    /// they arrive; publishes `RowsReceived` per batch. The completed run
    /// records the outcome but no `ResultSet` — the sink owns the rows.
    pub async fn execute_streaming(
        &self,
        sql: &str,
        connection_id: ConnectionId,
        sink: Arc<dyn RowSink>,
    ) -> Result<QueryRunId, ServiceError> {
        let (run_id, result) = self.run(sql, connection_id, sink).await;
        self.finish(run_id, result, None)
    }

    /// Shared execution path: registers the run, streams every batch to
    /// `sink`, returns `(columns, total_rows)` on success.
    async fn run(
        &self,
        sql: &str,
        connection_id: ConnectionId,
        sink: Arc<dyn RowSink>,
    ) -> (QueryRunId, Result<(Vec<ColumnMeta>, usize), ServiceError>) {
        let query = Query {
            id: tempr_domain::QueryId::new(),
            text: sql.to_string(),
            source_file: None,
            offset_start: 0,
            offset_end: sql.len(),
            fingerprint: [0u8; 32],
        };

        let run_id = QueryRunId::new();
        let query_run = QueryRun {
            id: run_id,
            query,
            connection_id,
            started_at: chrono::Utc::now(),
            finished_at: None,
            outcome: QueryOutcome::Success,
            result_set: None,
        };

        self.event_bus
            .publish(AppEvent::QueryStarted { run: run_id });
        self.active_runs.write().insert(
            run_id,
            ActiveRun {
                query_run,
                cancel_handle: None,
                cancelled: false,
            },
        );

        let sql_owned = sql.to_string();
        let result = self
            .connection_service
            .with_connection_fn(connection_id, |mut conn| {
                let sql = sql_owned.clone();
                let sink = sink.clone();
                async move {
                    // Captured before the (potentially long-running) execute
                    // call so a concurrent `cancel()` can reach this query
                    // without needing exclusive access to `conn`.
                    if let Some(active) = self.active_runs.write().get_mut(&run_id) {
                        active.cancel_handle = Some(conn.cancel_handle());
                    }

                    match conn.execute(&sql, &[]).await {
                        Ok(mut stream) => {
                            let columns = column_meta(stream.columns());
                            sink.columns(stream.columns());

                            let mut total_rows = 0usize;
                            let mut stream_err = None;
                            while let Some(batch_result) = stream.next_batch().await.transpose() {
                                match batch_result {
                                    Ok(batch) => {
                                        let count = batch.rows.len();
                                        total_rows += count;
                                        sink.batch(batch);
                                        self.event_bus
                                            .publish(AppEvent::RowsReceived { run: run_id, count });
                                    }
                                    Err(e) => {
                                        stream_err = Some(e);
                                        break;
                                    }
                                }
                            }

                            match stream_err {
                                Some(e) => (conn, Err(e)),
                                None => (conn, Ok((columns, total_rows))),
                            }
                        }
                        Err(e) => (conn, Err(e)),
                    }
                }
            })
            .await;

        (run_id, result)
    }

    /// Move the run from active to completed, publish `QueryFinished`, and
    /// map the result to the public return type. A run flagged by `cancel()`
    /// completes as `Cancelled` and returns `Ok(run_id)` — partial rows the
    /// sink already received stay valid.
    fn finish(
        &self,
        run_id: QueryRunId,
        result: Result<(Vec<ColumnMeta>, usize), ServiceError>,
        result_set: Option<ResultSet>,
    ) -> Result<QueryRunId, ServiceError> {
        // Take the entry under the lock, then release it before touching
        // `completed_runs` or publishing — handlers may call back into us.
        let removed = self.active_runs.write().remove(&run_id);
        let Some(mut active) = removed else {
            return Self::map_result(result, run_id);
        };

        let cancelled = active.cancelled;
        let outcome = if cancelled {
            QueryOutcome::Cancelled
        } else {
            match &result {
                Ok(_) => QueryOutcome::Success,
                Err(e) => QueryOutcome::Error(e.to_string()),
            }
        };
        active.query_run.outcome = outcome.clone();
        active.query_run.finished_at = Some(chrono::Utc::now());
        active.query_run.result_set = result_set;
        self.completed_runs.write().insert(run_id, active.query_run);
        self.event_bus.publish(AppEvent::QueryFinished {
            run: run_id,
            outcome,
        });

        if cancelled {
            Ok(run_id)
        } else {
            Self::map_result(result, run_id)
        }
    }

    fn map_result(
        result: Result<(Vec<ColumnMeta>, usize), ServiceError>,
        run_id: QueryRunId,
    ) -> Result<QueryRunId, ServiceError> {
        result.map(|_| run_id).map_err(|e| match e {
            ServiceError::QueryFailed { .. } => e,
            other => ServiceError::QueryFailed {
                name: "QueryService",
                reason: other.to_string(),
            },
        })
    }

    /// Cancel a run. For an in-flight run the driver-side cancel is issued and
    /// the run is flagged so its `finish()` records `Cancelled`; for an
    /// unknown/finished run a `QueryFinished { Cancelled }` is published
    /// immediately so callers always observe a terminal event.
    pub async fn cancel(&self, run_id: QueryRunId) -> Result<(), ServiceError> {
        let handle = {
            let mut active_runs = self.active_runs.write();
            match active_runs.get_mut(&run_id) {
                Some(active) => {
                    active.cancelled = true;
                    active.cancel_handle.take()
                }
                None => {
                    drop(active_runs);
                    self.event_bus.publish(AppEvent::QueryFinished {
                        run: run_id,
                        outcome: QueryOutcome::Cancelled,
                    });
                    return Ok(());
                }
            }
        };

        if let Some(handle) = handle
            && let Err(e) = handle.cancel().await
        {
            tracing::warn!("failed to cancel query {run_id:?} on driver: {e}");
        }
        Ok(())
    }

    pub fn active_runs(&self) -> Vec<QueryRunId> {
        self.active_runs.read().keys().copied().collect()
    }

    pub fn completed_run(&self, run_id: QueryRunId) -> Option<QueryRun> {
        self.completed_runs.read().get(&run_id).cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use parking_lot::Mutex;
    use tempr_events::EventFilter;

    fn make_event_bus() -> Arc<EventBus> {
        Arc::new(EventBus::new())
    }

    #[tokio::test]
    async fn active_runs_empty_initially() {
        let bus = make_event_bus();
        let cs = ConnectionService::new(bus.clone());
        let svc = QueryService::new(bus, cs);
        assert!(svc.active_runs().is_empty());
    }

    #[tokio::test]
    async fn execute_fails_without_connection() {
        let bus = make_event_bus();
        let cs = ConnectionService::new(bus.clone());
        let svc = QueryService::new(bus, cs);
        let id = ConnectionId::new();
        let result = svc.execute("SELECT 1", id).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn cancel_publishes_event() {
        let bus = make_event_bus();
        let received: Arc<Mutex<Vec<tempr_events::AppEventKind>>> =
            Arc::new(Mutex::new(Vec::new()));
        let r = received.clone();
        let _sub = bus.subscribe(EventFilter::All, move |event| {
            r.lock().push(event.kind());
        });

        let cs = ConnectionService::new(bus.clone());
        let svc = QueryService::new(bus, cs);
        let run_id = QueryRunId::new();

        svc.cancel(run_id).await.unwrap();

        let events = received.lock();
        assert!(
            events
                .iter()
                .any(|e| matches!(e, tempr_events::AppEventKind::QueryFinished)),
            "expected QueryFinished event after cancel"
        );
    }

    #[tokio::test]
    async fn completed_run_not_stored_without_execute() {
        let bus = make_event_bus();
        let cs = ConnectionService::new(bus.clone());
        let svc = QueryService::new(bus, cs);
        let run_id = QueryRunId::new();
        assert!(svc.completed_run(run_id).is_none());
    }

    // ── Streaming tests with a mock driver ─────────────────────────────

    use tempr_db::{DatabaseDriver, DriverConnection, DriverError, EngineId, QueryStreamImpl};
    use tempr_domain::{Connection, DriverKind, SecretRef};

    struct NoopCancel;
    #[async_trait::async_trait]
    impl CancelHandle for NoopCancel {
        async fn cancel(&self) -> Result<(), DriverError> {
            Ok(())
        }
    }

    /// Optional gate: after the first batch the stream waits for `notify`
    /// and then reports `DriverError::Cancelled` (a server-side cancel).
    struct MockStream {
        columns: Vec<ColumnSpec>,
        batches: std::collections::VecDeque<Batch>,
        gate: Option<Arc<tokio::sync::Notify>>,
        yielded: usize,
    }
    #[async_trait::async_trait]
    impl QueryStreamImpl for MockStream {
        fn columns(&self) -> &[ColumnSpec] {
            &self.columns
        }
        async fn next_batch(&mut self) -> Result<Option<Batch>, DriverError> {
            if self.yielded >= 1
                && let Some(gate) = &self.gate
            {
                gate.notified().await;
                return Err(DriverError::Cancelled);
            }
            self.yielded += 1;
            Ok(self.batches.pop_front())
        }
        fn rows_affected(&self) -> u64 {
            0
        }
    }

    /// Yields `batches` of `rows_per_batch` single-int rows.
    struct MockConn {
        batches: usize,
        rows_per_batch: usize,
        gate: Option<Arc<tokio::sync::Notify>>,
    }
    #[async_trait::async_trait]
    impl DriverConnection for MockConn {
        async fn execute(
            &mut self,
            _sql: &str,
            _params: &[Value],
        ) -> Result<tempr_db::QueryStream, DriverError> {
            let columns = vec![ColumnSpec {
                name: "n".into(),
                ordinal: 0,
                data_type: "int8".into(),
                value_type: tempr_domain::ValueType::Int,
                nullable: false,
                table_schema: None,
                table_name: None,
            }];
            let batches = (0..self.batches)
                .map(|b| Batch {
                    rows: (0..self.rows_per_batch)
                        .map(|r| vec![Value::Int8((b * self.rows_per_batch + r) as i64)])
                        .collect(),
                    batch_index: b,
                })
                .collect();
            Ok(tempr_db::QueryStream::new(
                Box::new(MockStream {
                    columns,
                    batches,
                    gate: self.gate.clone(),
                    yielded: 0,
                }),
                self.rows_per_batch,
            ))
        }
        async fn cancel(&mut self) -> Result<(), DriverError> {
            Ok(())
        }
        fn cancel_handle(&self) -> Box<dyn CancelHandle> {
            Box::new(NoopCancel)
        }
        async fn snapshot_schema(
            &mut self,
            _scope: tempr_db::SchemaScope,
        ) -> Result<Vec<tempr_db::SchemaSnapshotEntry>, DriverError> {
            Ok(Vec::new())
        }
    }

    struct MockDriver {
        batches: usize,
        rows_per_batch: usize,
        gate: Option<Arc<tokio::sync::Notify>>,
    }
    #[async_trait::async_trait]
    impl DatabaseDriver for MockDriver {
        fn engine(&self) -> EngineId {
            EngineId(DriverKind::Postgres.engine_name().to_string())
        }
        async fn connect(
            &self,
            _connection: &Connection,
        ) -> Result<Box<dyn DriverConnection>, DriverError> {
            Ok(Box::new(MockConn {
                batches: self.batches,
                rows_per_batch: self.rows_per_batch,
                gate: self.gate.clone(),
            }))
        }
    }

    async fn connected_service(
        batches: usize,
        rows_per_batch: usize,
    ) -> (Arc<EventBus>, Arc<QueryService>, ConnectionId) {
        connected_service_gated(batches, rows_per_batch, None).await
    }

    async fn connected_service_gated(
        batches: usize,
        rows_per_batch: usize,
        gate: Option<Arc<tokio::sync::Notify>>,
    ) -> (Arc<EventBus>, Arc<QueryService>, ConnectionId) {
        let bus = make_event_bus();
        let cs = ConnectionService::new(bus.clone());
        cs.register_driver(Arc::new(MockDriver {
            batches,
            rows_per_batch,
            gate,
        }));
        let id = ConnectionId::new();
        let conn = Connection {
            id,
            name: "mock".into(),
            driver: DriverKind::Postgres,
            host: "localhost".into(),
            port: 5432,
            database: "db".into(),
            username: "u".into(),
            password: "p".into(),
            secret_ref: SecretRef {
                vault_key: "k".into(),
            },
        };
        cs.connect(&conn).await.unwrap();
        (bus.clone(), QueryService::new(bus, cs), id)
    }

    #[derive(Default)]
    struct RecordingSink {
        columns: Mutex<Vec<ColumnSpec>>,
        batches: Mutex<Vec<Batch>>,
    }
    impl RowSink for RecordingSink {
        fn columns(&self, columns: &[ColumnSpec]) {
            *self.columns.lock() = columns.to_vec();
        }
        fn batch(&self, batch: Batch) {
            self.batches.lock().push(batch);
        }
    }

    #[tokio::test]
    async fn execute_streaming_delivers_columns_then_batches_in_order() {
        let (bus, svc, id) = connected_service(3, 4).await;
        let events: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let e = events.clone();
        let _sub = bus.subscribe(EventFilter::All, move |ev| {
            e.lock().push(match ev {
                AppEvent::QueryStarted { .. } => "started".to_string(),
                AppEvent::RowsReceived { count, .. } => format!("rows:{count}"),
                AppEvent::QueryFinished { outcome, .. } => format!("finished:{outcome:?}"),
                _ => "other".to_string(),
            });
        });

        let sink = Arc::new(RecordingSink::default());
        let run_id = svc
            .execute_streaming("SELECT n", id, sink.clone())
            .await
            .unwrap();

        assert_eq!(sink.columns.lock().len(), 1);
        assert_eq!(sink.columns.lock()[0].name, "n");
        let batches = sink.batches.lock();
        assert_eq!(batches.len(), 3);
        assert_eq!(batches[2].rows[3][0], Value::Int8(11));

        assert_eq!(
            *events.lock(),
            vec!["started", "rows:4", "rows:4", "rows:4", "finished:Success"]
        );

        let run = svc.completed_run(run_id).unwrap();
        assert_eq!(run.outcome, QueryOutcome::Success);
        assert!(
            run.result_set.is_none(),
            "streaming runs do not retain rows"
        );
        assert!(svc.active_runs().is_empty());
    }

    #[tokio::test]
    async fn execute_collects_all_rows_and_publishes_rows_received() {
        let (bus, svc, id) = connected_service(2, 5).await;
        let counts: Arc<Mutex<Vec<usize>>> = Arc::new(Mutex::new(Vec::new()));
        let c = counts.clone();
        let _sub = bus.subscribe(EventFilter::All, move |ev| {
            if let AppEvent::RowsReceived { count, .. } = ev {
                c.lock().push(*count);
            }
        });

        let run_id = svc.execute("SELECT n", id).await.unwrap();
        let rs = svc.completed_run(run_id).unwrap().result_set.unwrap();
        assert_eq!(rs.rows.len(), 10);
        assert_eq!(rs.total_rows, 10);
        assert_eq!(rs.columns[0].name, "n");
        assert_eq!(*counts.lock(), vec![5, 5]);
    }

    #[tokio::test]
    async fn cancel_during_run_completes_as_cancelled_not_error() {
        let gate = Arc::new(tokio::sync::Notify::new());
        let (bus, svc, id) = connected_service_gated(3, 2, Some(gate.clone())).await;
        let finished: Arc<Mutex<Vec<QueryOutcome>>> = Arc::new(Mutex::new(Vec::new()));
        let f = finished.clone();
        let _sub = bus.subscribe(EventFilter::All, move |ev| {
            if let AppEvent::QueryFinished { outcome, .. } = ev {
                f.lock().push(outcome.clone());
            }
        });

        let sink = Arc::new(RecordingSink::default());
        let svc2 = svc.clone();
        let sink2 = sink.clone();
        let task = tokio::spawn(async move { svc2.execute_streaming("SELECT n", id, sink2).await });

        // Wait until the first batch has been delivered, i.e. the run is in flight.
        while sink.batches.lock().is_empty() {
            tokio::time::sleep(std::time::Duration::from_millis(1)).await;
        }
        let run_id = svc.active_runs()[0];
        svc.cancel(run_id).await.unwrap();
        gate.notify_one();

        let result = task.await.unwrap();
        assert_eq!(result.unwrap(), run_id, "a cancelled run is not an error");
        let run = svc
            .completed_run(run_id)
            .expect("cancelled run is recorded");
        assert_eq!(run.outcome, QueryOutcome::Cancelled);
        assert_eq!(*finished.lock(), vec![QueryOutcome::Cancelled]);
        assert_eq!(
            sink.batches.lock().len(),
            1,
            "partial rows stay with the sink"
        );
        assert!(svc.active_runs().is_empty());
    }
}
