use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::RwLock;
use tempr_db::{SchemaScope, SchemaSnapshotEntry};
use tempr_domain::{ConnectionId, SchemaObject, SchemaObjectId, SchemaSnapshot, SchemaSnapshotId};
use tempr_events::{AppEvent, EventBus};

use crate::ServiceError;
use crate::connection::ConnectionService;

pub struct SchemaService {
    event_bus: Arc<EventBus>,
    connection_service: Arc<ConnectionService>,
    snapshots: RwLock<HashMap<ConnectionId, Arc<SchemaSnapshot>>>,
}

impl SchemaService {
    pub fn new(event_bus: Arc<EventBus>, connection_service: Arc<ConnectionService>) -> Arc<Self> {
        Arc::new(Self {
            event_bus,
            connection_service,
            snapshots: RwLock::new(HashMap::new()),
        })
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
            .await?;

        // First pass: collect all tables and build a name→id lookup
        let mut table_ids: HashMap<(String, String), SchemaObjectId> = HashMap::new();
        let mut objects: Vec<SchemaObject> = Vec::new();

        for entry in &entries {
            match entry {
                SchemaSnapshotEntry::Table { schema, name, .. } => {
                    let id = SchemaObjectId::new();
                    table_ids.insert((schema.clone(), name.clone()), id);
                    objects.push(SchemaObject::Table {
                        id,
                        schema: schema.clone(),
                        name: name.clone(),
                        estimated_rows: None,
                    });
                }
                SchemaSnapshotEntry::View { schema, name, .. } => {
                    let id = SchemaObjectId::new();
                    table_ids.insert((schema.clone(), name.clone()), id);
                    objects.push(SchemaObject::View {
                        id,
                        schema: schema.clone(),
                        name: name.clone(),
                        definition: String::new(),
                    });
                }
                _ => {}
            }
        }

        // Second pass: columns and indexes reference their parent table
        for entry in &entries {
            let id = SchemaObjectId::new();
            match entry {
                SchemaSnapshotEntry::Column {
                    parent_schema,
                    parent_table,
                    name,
                    data_type,
                    nullable,
                    ordinal,
                    default,
                } => {
                    let parent_id = table_ids
                        .get(&(parent_schema.clone(), parent_table.clone()))
                        .copied()
                        .unwrap_or_else(SchemaObjectId::new);
                    objects.push(SchemaObject::Column {
                        id,
                        parent_id,
                        name: name.clone(),
                        data_type: data_type.clone(),
                        nullable: *nullable,
                        ordinal: *ordinal,
                        default: default.clone(),
                    });
                }
                SchemaSnapshotEntry::Index {
                    parent_schema,
                    parent_table,
                    name,
                    columns,
                    unique,
                    index_type,
                } => {
                    let parent_table_id = table_ids
                        .get(&(parent_schema.clone(), parent_table.clone()))
                        .copied()
                        .unwrap_or_else(SchemaObjectId::new);
                    objects.push(SchemaObject::Index {
                        id,
                        parent_table_id,
                        name: name.clone(),
                        columns: columns.clone(),
                        unique: *unique,
                        index_type: index_type.clone(),
                    });
                }
                _ => {}
            }
        }

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

#[cfg(test)]
mod tests {
    use super::*;
    use tempr_domain::ConnectionId;

    fn make_event_bus() -> Arc<EventBus> {
        Arc::new(EventBus::new())
    }

    #[tokio::test]
    async fn snapshot_returns_none_for_unknown() {
        let bus = make_event_bus();
        let cs = ConnectionService::new(bus.clone());
        let svc = SchemaService::new(bus, cs);
        let id = ConnectionId::new();
        assert!(svc.snapshot(id).is_none());
    }

    #[tokio::test]
    async fn version_returns_none_for_unknown() {
        let bus = make_event_bus();
        let cs = ConnectionService::new(bus.clone());
        let svc = SchemaService::new(bus, cs);
        let id = ConnectionId::new();
        assert!(svc.version(id).is_none());
    }

    #[tokio::test]
    async fn refresh_fails_without_connection() {
        let bus = make_event_bus();
        let cs = ConnectionService::new(bus.clone());
        let svc = SchemaService::new(bus, cs);
        let id = ConnectionId::new();
        let result = svc.refresh(id).await;
        assert!(result.is_err());
    }
}
