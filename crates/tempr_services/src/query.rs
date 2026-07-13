use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::RwLock;
use tempr_domain::{
    ColumnMeta, ConnectionId, Query, QueryOutcome, QueryRun, QueryRunId, ResultSet, Value,
};
use tempr_events::{AppEvent, EventBus};

use crate::ServiceError;
use crate::connection::ConnectionService;

struct ActiveRun {
    query_run: QueryRun,
}

pub struct QueryService {
    event_bus: Arc<EventBus>,
    connection_service: Arc<ConnectionService>,
    active_runs: RwLock<HashMap<QueryRunId, ActiveRun>>,
}

impl QueryService {
    pub fn new(event_bus: Arc<EventBus>, connection_service: Arc<ConnectionService>) -> Arc<Self> {
        Arc::new(Self {
            event_bus,
            connection_service,
            active_runs: RwLock::new(HashMap::new()),
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
        self.active_runs
            .write()
            .insert(run_id, ActiveRun { query_run });

        let sql_owned = sql.to_string();
        let result = self
            .connection_service
            .with_connection_fn(connection_id, |mut conn| {
                let sql = sql_owned.clone();
                async move {
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
        if let Some(active) = active_runs.get_mut(&run_id) {
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
        }
        active_runs.remove(&run_id);

        result
            .map(|_| run_id)
            .map_err(|e| ServiceError::StartupFailed {
                name: "QueryService",
                reason: e.to_string(),
            })
    }

    pub async fn cancel(&self, run_id: QueryRunId) -> Result<(), ServiceError> {
        self.active_runs.write().remove(&run_id);
        self.event_bus.publish(AppEvent::QueryFinished {
            run: run_id,
            outcome: QueryOutcome::Cancelled,
        });
        Ok(())
    }

    pub fn active_runs(&self) -> Vec<QueryRunId> {
        self.active_runs.read().keys().copied().collect()
    }
}
