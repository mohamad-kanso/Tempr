use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::RwLock;
use tempr_db::{DatabaseDriver, SchemaScope, SchemaSnapshotEntry};
use tempr_domain::{ConnectionId, SchemaObject, SchemaObjectId, SchemaSnapshot, SchemaSnapshotId};
use tempr_events::{AppEvent, EventBus};

use crate::ServiceError;
use crate::connection::ConnectionService;

pub struct SchemaService {
    event_bus: Arc<EventBus>,
    connection_service: Arc<ConnectionService>,
    snapshots: RwLock<HashMap<ConnectionId, Arc<SchemaSnapshot>>>,
    drivers: RwLock<HashMap<String, Arc<dyn DatabaseDriver>>>,
}

impl SchemaService {
    pub fn new(event_bus: Arc<EventBus>, connection_service: Arc<ConnectionService>) -> Arc<Self> {
        Arc::new(Self {
            event_bus,
            connection_service,
            snapshots: RwLock::new(HashMap::new()),
            drivers: RwLock::new(HashMap::new()),
        })
    }

    pub fn register_driver(&self, driver: Arc<dyn DatabaseDriver>) {
        let engine = driver.engine().0.clone();
        self.drivers.write().insert(engine, driver);
    }

    pub async fn refresh(
        &self,
        connection_id: ConnectionId,
    ) -> Result<Arc<SchemaSnapshot>, ServiceError> {
        let entries = self
            .connection_service
            .with_connection_fn(connection_id, |mut conn| async move {
                let result = conn.snapshot_schema(SchemaScope::All).await;
                match result {
                    Ok(entries) => (conn, Ok(entries)),
                    Err(e) => (conn, Err(e)),
                }
            })
            .await
            .map_err(|e| ServiceError::StartupFailed {
                name: "SchemaService",
                reason: e.to_string(),
            })?;

        let objects = entries
            .into_iter()
            .map(|entry| {
                let id = SchemaObjectId::new();
                match entry {
                    SchemaSnapshotEntry::Table {
                        schema,
                        name,
                        estimated_rows,
                    } => SchemaObject::Table {
                        id,
                        schema,
                        name,
                        estimated_rows,
                    },
                    SchemaSnapshotEntry::View {
                        schema,
                        name,
                        definition,
                    } => SchemaObject::View {
                        id,
                        schema,
                        name,
                        definition,
                    },
                    SchemaSnapshotEntry::Column {
                        parent_schema: _,
                        parent_table: _,
                        name,
                        data_type,
                        nullable,
                        ordinal,
                        default,
                    } => SchemaObject::Column {
                        id,
                        parent_id: SchemaObjectId::new(),
                        name,
                        data_type,
                        nullable,
                        ordinal,
                        default,
                    },
                    SchemaSnapshotEntry::Index {
                        parent_schema: _,
                        parent_table: _,
                        name,
                        columns,
                        unique,
                        index_type,
                    } => SchemaObject::Index {
                        id,
                        parent_table_id: SchemaObjectId::new(),
                        name,
                        columns,
                        unique,
                        index_type,
                    },
                }
            })
            .collect();

        let version = {
            let snapshots = self.snapshots.read();
            snapshots
                .get(&connection_id)
                .map(|s| s.version + 1)
                .unwrap_or(1)
        };

        let snapshot = Arc::new(SchemaSnapshot {
            id: SchemaSnapshotId::new(),
            connection_id,
            version,
            fetched_at: chrono::Utc::now(),
            objects,
        });

        self.snapshots
            .write()
            .insert(connection_id, snapshot.clone());

        self.event_bus.publish(AppEvent::SchemaRefreshed {
            connection: connection_id,
            snapshot: snapshot.id,
        });

        Ok(snapshot)
    }

    pub fn snapshot(&self, connection_id: ConnectionId) -> Option<Arc<SchemaSnapshot>> {
        self.snapshots.read().get(&connection_id).cloned()
    }

    pub fn version(&self, connection_id: ConnectionId) -> Option<u64> {
        self.snapshots.read().get(&connection_id).map(|s| s.version)
    }
}
