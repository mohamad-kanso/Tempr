use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::RwLock;
use tempr_db::CancelHandle;
use tempr_domain::{
    ColumnMeta, ConnectionId, Query, QueryOutcome, QueryRun, QueryRunId, ResultSet, Value,
};
use tempr_events::{AppEvent, EventBus};

use crate::ServiceError;
use crate::connection::ConnectionService;

struct ActiveRun {
    query_run: QueryRun,
    cancel_handle: Option<Box<dyn CancelHandle>>,
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

    pub async fn execute(
        &self,
        sql: &str,
        connection_id: ConnectionId,
    ) -> Result<QueryRunId, ServiceError> {
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
            query: query.clone(),
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
            },
        );

        let sql_owned = sql.to_string();
        let result = self
            .connection_service
            .with_connection_fn(connection_id, |mut conn| {
                let sql = sql_owned.clone();
                async move {
                    // Captured before the (potentially long-running) execute
                    // call so a concurrent `cancel()` can reach this query
                    // without needing exclusive access to `conn`.
                    if let Some(active) = self.active_runs.write().get_mut(&run_id) {
                        active.cancel_handle = Some(conn.cancel_handle());
                    }

                    match conn.execute(&sql, &[]).await {
                        Ok(mut stream) => {
                            let columns: Vec<ColumnMeta> = stream
                                .columns()
                                .iter()
                                .map(|c| ColumnMeta {
                                    name: c.name.clone(),
                                    data_type: c.data_type.clone(),
                                    nullable: c.nullable,
                                    ordinal: c.ordinal,
                                })
                                .collect();

                            let mut all_rows: Vec<Vec<Value>> = Vec::new();
                            let mut stream_err = None;
                            while let Some(batch_result) = stream.next_batch().await.transpose() {
                                match batch_result {
                                    Ok(batch) => {
                                        all_rows.extend(batch.rows);
                                    }
                                    Err(e) => {
                                        stream_err = Some(e);
                                        break;
                                    }
                                }
                            }

                            if let Some(e) = stream_err {
                                (conn, Err(e))
                            } else {
                                let result_set = ResultSet {
                                    columns,
                                    rows: all_rows.clone(),
                                    total_rows: all_rows.len(),
                                    truncated: false,
                                };
                                (conn, Ok(result_set))
                            }
                        }
                        Err(e) => (conn, Err(e)),
                    }
                }
            })
            .await;

        let mut active_runs = self.active_runs.write();
        if let Some(mut active) = active_runs.remove(&run_id) {
            match &result {
                Ok(result_set) => {
                    active.query_run.outcome = QueryOutcome::Success;
                    active.query_run.finished_at = Some(chrono::Utc::now());
                    active.query_run.result_set = Some(result_set.clone());
                    self.event_bus.publish(AppEvent::QueryFinished {
                        run: run_id,
                        outcome: QueryOutcome::Success,
                    });
                }
                Err(e) => {
                    active.query_run.outcome = QueryOutcome::Error(e.to_string());
                    active.query_run.finished_at = Some(chrono::Utc::now());
                    self.event_bus.publish(AppEvent::QueryFinished {
                        run: run_id,
                        outcome: QueryOutcome::Error(e.to_string()),
                    });
                }
            }
            self.completed_runs.write().insert(run_id, active.query_run);
        }

        result
            .map(|_| run_id)
            .map_err(|e| ServiceError::QueryFailed {
                name: "QueryService",
                reason: e.to_string(),
            })
    }

    pub async fn cancel(&self, run_id: QueryRunId) -> Result<(), ServiceError> {
        let handle = self
            .active_runs
            .write()
            .remove(&run_id)
            .and_then(|active| active.cancel_handle);

        if let Some(handle) = handle
            && let Err(e) = handle.cancel().await
        {
            tracing::warn!("failed to cancel query {run_id:?} on driver: {e}");
        }

        self.event_bus.publish(AppEvent::QueryFinished {
            run: run_id,
            outcome: QueryOutcome::Cancelled,
        });
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
}
