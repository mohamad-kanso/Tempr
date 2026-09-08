use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::RwLock;
use tempr_db::{DriverError, ObjectKind, SchemaScope, SchemaSnapshotEntry};
use tempr_domain::{
    ConnectionId, SchemaFingerprintRecord, SchemaObject, SchemaObjectId, SchemaObjectKind,
    SchemaSnapshot, SchemaSnapshotId,
};
use tempr_events::{AppEvent, EventBus};
use tempr_workspace::Storage;

use crate::connection::ConnectionService;
use crate::{Service, ServiceError};

pub struct SchemaService {
    event_bus: Arc<EventBus>,
    connection_service: Arc<ConnectionService>,
    snapshots: RwLock<HashMap<ConnectionId, Arc<SchemaSnapshot>>>,
    /// Absent when no workspace root is writable: the catalog then lives only
    /// in memory for the session, which is a degradation, never an error.
    storage: Option<Arc<dyn Storage>>,
}

impl SchemaService {
    pub fn new(event_bus: Arc<EventBus>, connection_service: Arc<ConnectionService>) -> Arc<Self> {
        Arc::new(Self {
            event_bus,
            connection_service,
            snapshots: RwLock::new(HashMap::new()),
            storage: None,
        })
    }

    /// Same service, backed by a catalog cache on disk.
    pub fn with_cache(
        event_bus: Arc<EventBus>,
        connection_service: Arc<ConnectionService>,
        storage: Arc<dyn Storage>,
    ) -> Arc<Self> {
        Arc::new(Self {
            event_bus,
            connection_service,
            snapshots: RwLock::new(HashMap::new()),
            storage: Some(storage),
        })
    }

    /// Load this connection's cached snapshot, if any, into memory. Returns
    /// what it loaded so a caller can use the catalog before — or without —
    /// reaching the database.
    pub async fn load_cached(&self, connection_id: ConnectionId) -> Option<Arc<SchemaSnapshot>> {
        let storage = self.storage.as_ref()?;
        let cache = storage.catalog_cache(connection_id);
        match cache.load().await {
            Ok(Some(snapshot)) => {
                let snapshot = Arc::new(snapshot);
                self.snapshots
                    .write()
                    .insert(connection_id, snapshot.clone());
                Some(snapshot)
            }
            Ok(None) => None,
            Err(e) => {
                tracing::warn!(error = %e, "catalog cache load failed; will re-introspect");
                None
            }
        }
    }

    /// Best-effort cache write. Introspection already succeeded, so a failure
    /// here is logged and swallowed — never propagated to the caller.
    async fn save_cached(&self, snapshot: &SchemaSnapshot) {
        let Some(storage) = self.storage.as_ref() else {
            return;
        };
        let cache = storage.catalog_cache(snapshot.connection_id);
        // Skip the write when the content is byte-identical to what is there.
        if let Ok(Some(existing)) = cache.load().await
            && let (Ok(a), Ok(b)) = (
                tempr_workspace::encode_catalog(&existing),
                tempr_workspace::encode_catalog(snapshot),
            )
            && tempr_workspace::content_hash(&a) == tempr_workspace::content_hash(&b)
        {
            return;
        }
        if let Err(e) = cache.save(snapshot).await {
            tracing::warn!(error = %e, "catalog cache save failed; catalog stays in memory");
        }
    }

    pub async fn refresh(
        &self,
        connection_id: ConnectionId,
    ) -> Result<Arc<SchemaSnapshot>, ServiceError> {
        // One connection, one moment: entries, keywords and fingerprints must
        // describe the same server state or the first diff is already wrong.
        let (entries, keywords, fingerprints) = self
            .connection_service
            .with_metadata_connection_fn(connection_id, |mut conn| async move {
                let entries = conn.snapshot_schema(SchemaScope::SearchPath).await?;
                let keywords = conn.keywords().await?;
                let fingerprints = match conn.schema_fingerprints(SchemaScope::SearchPath).await {
                    Ok(f) => f,
                    // A driver without a sweep simply never refreshes
                    // incrementally; that is not a refresh failure.
                    Err(DriverError::Unsupported(_)) => Vec::new(),
                    Err(e) => return Err(e),
                };
                Ok((entries, keywords, fingerprints))
            })
            .await?;

        let objects = Self::objects_from_entries(connection_id, &entries);
        let fingerprints = fingerprints
            .into_iter()
            .map(|f| SchemaFingerprintRecord {
                kind: domain_kind(f.kind),
                native_id: f.native_id,
                version: f.version,
            })
            .collect();

        let version = self
            .snapshots
            .read()
            .get(&connection_id)
            .map(|s| s.version + 1)
            .unwrap_or(1);

        let snapshot = Arc::new(SchemaSnapshot {
            id: SchemaSnapshotId::new(),
            connection_id,
            version,
            fetched_at: chrono::Utc::now(),
            objects,
            keywords,
            fingerprints,
        });

        self.snapshots
            .write()
            .insert(connection_id, snapshot.clone());
        self.save_cached(&snapshot).await;
        self.event_bus.publish(AppEvent::SchemaRefreshed {
            connection: connection_id,
            snapshot: snapshot.id,
        });
        Ok(snapshot)
    }

    /// Map driver entries to domain objects, deriving every id from
    /// `(connection, kind, native_id)` so the result is reproducible.
    fn objects_from_entries(
        connection_id: ConnectionId,
        entries: &[SchemaSnapshotEntry],
    ) -> Vec<SchemaObject> {
        // First pass: relations, and a (schema, name) → id map for parents.
        let mut parents: HashMap<(String, String), SchemaObjectId> = HashMap::new();
        let mut objects: Vec<SchemaObject> = Vec::new();

        for entry in entries {
            match entry {
                SchemaSnapshotEntry::Table {
                    native_id,
                    schema,
                    name,
                    estimated_rows,
                } => {
                    let id =
                        SchemaObjectId::derived(connection_id, SchemaObjectKind::Table, *native_id);
                    parents.insert((schema.clone(), name.clone()), id);
                    objects.push(SchemaObject::Table {
                        id,
                        schema: schema.clone(),
                        name: name.clone(),
                        estimated_rows: *estimated_rows,
                    });
                }
                SchemaSnapshotEntry::View {
                    native_id,
                    schema,
                    name,
                    definition,
                } => {
                    let id =
                        SchemaObjectId::derived(connection_id, SchemaObjectKind::View, *native_id);
                    parents.insert((schema.clone(), name.clone()), id);
                    objects.push(SchemaObject::View {
                        id,
                        schema: schema.clone(),
                        name: name.clone(),
                        definition: definition.clone(),
                    });
                }
                _ => {}
            }
        }

        // Second pass: children, resolved against the map above.
        for entry in entries {
            match entry {
                SchemaSnapshotEntry::Column {
                    native_id,
                    parent_schema,
                    parent_table,
                    name,
                    data_type,
                    nullable,
                    ordinal,
                    default,
                } => {
                    let Some(parent_id) =
                        parents.get(&(parent_schema.clone(), parent_table.clone()))
                    else {
                        // A column whose relation is out of scope has no
                        // parent to hang from; dropping it is correct, and
                        // inventing a parent id (as this code used to) is not.
                        tracing::debug!(
                            schema = %parent_schema, table = %parent_table, column = %name,
                            "column skipped: parent relation not in scope"
                        );
                        continue;
                    };
                    objects.push(SchemaObject::Column {
                        id: SchemaObjectId::derived(
                            connection_id,
                            SchemaObjectKind::Column,
                            *native_id,
                        ),
                        parent_id: *parent_id,
                        name: name.clone(),
                        data_type: data_type.clone(),
                        nullable: *nullable,
                        ordinal: *ordinal,
                        default: default.clone(),
                    });
                }
                SchemaSnapshotEntry::Index {
                    native_id,
                    parent_schema,
                    parent_table,
                    name,
                    columns,
                    unique,
                    index_type,
                } => {
                    let Some(parent_table_id) =
                        parents.get(&(parent_schema.clone(), parent_table.clone()))
                    else {
                        continue;
                    };
                    objects.push(SchemaObject::Index {
                        id: SchemaObjectId::derived(
                            connection_id,
                            SchemaObjectKind::Index,
                            *native_id,
                        ),
                        parent_table_id: *parent_table_id,
                        name: name.clone(),
                        columns: columns.clone(),
                        unique: *unique,
                        index_type: index_type.clone(),
                    });
                }
                SchemaSnapshotEntry::Function {
                    native_id,
                    schema,
                    name,
                    parameters,
                    return_type,
                    language,
                } => {
                    objects.push(SchemaObject::Function {
                        id: SchemaObjectId::derived(
                            connection_id,
                            SchemaObjectKind::Function,
                            *native_id,
                        ),
                        schema: schema.clone(),
                        name: name.clone(),
                        parameters: parameters.clone(),
                        return_type: return_type.clone(),
                        language: language.clone(),
                    });
                }
                _ => {}
            }
        }
        objects
    }

    pub fn snapshot(&self, connection_id: ConnectionId) -> Option<Arc<SchemaSnapshot>> {
        self.snapshots.read().get(&connection_id).cloned()
    }

    pub fn version(&self, connection_id: ConnectionId) -> Option<u64> {
        self.snapshots.read().get(&connection_id).map(|s| s.version)
    }
}

/// The sweep cannot tell a table from a view — both are `pg_class` rows —
/// while the domain can. Mapping `Relation` to `Table` is safe because a
/// fingerprint key is only ever compared against another fingerprint key.
fn domain_kind(kind: ObjectKind) -> SchemaObjectKind {
    match kind {
        ObjectKind::Relation => SchemaObjectKind::Table,
        ObjectKind::Column => SchemaObjectKind::Column,
        ObjectKind::Index => SchemaObjectKind::Index,
        ObjectKind::Function => SchemaObjectKind::Function,
    }
}

#[async_trait::async_trait]
impl Service for SchemaService {
    fn name(&self) -> &'static str {
        "SchemaService"
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

    fn table_entry(native_id: u64, schema: &str, name: &str) -> SchemaSnapshotEntry {
        SchemaSnapshotEntry::Table {
            native_id,
            schema: schema.to_string(),
            name: name.to_string(),
            estimated_rows: Some(42),
        }
    }

    #[test]
    fn objects_from_entries_derives_stable_ids_and_links_children() {
        let connection = ConnectionId::new();
        let entries = vec![
            table_entry(16384, "public", "users"),
            SchemaSnapshotEntry::View {
                native_id: 16385,
                schema: "public".to_string(),
                name: "active_users".to_string(),
                definition: "SELECT 1".to_string(),
            },
            SchemaSnapshotEntry::Column {
                native_id: (16384u64 << 16) | 1,
                parent_schema: "public".to_string(),
                parent_table: "users".to_string(),
                name: "id".to_string(),
                data_type: "bigint".to_string(),
                nullable: false,
                ordinal: 1,
                default: None,
            },
        ];

        let first = SchemaService::objects_from_entries(connection, &entries);
        let second = SchemaService::objects_from_entries(connection, &entries);
        assert_eq!(
            first.iter().map(|o| o.id()).collect::<Vec<_>>(),
            second.iter().map(|o| o.id()).collect::<Vec<_>>(),
            "mapping the same entries twice must produce the same ids"
        );

        let table_id = SchemaObjectId::derived(connection, SchemaObjectKind::Table, 16384);
        let column_parent = first
            .iter()
            .find_map(|o| match o {
                SchemaObject::Column {
                    parent_id, name, ..
                } if name == "id" => Some(*parent_id),
                _ => None,
            })
            .expect("column missing");
        assert_eq!(
            column_parent, table_id,
            "column must hang off its real table id"
        );

        let rows = first.iter().find_map(|o| match o {
            SchemaObject::Table { estimated_rows, .. } => Some(*estimated_rows),
            _ => None,
        });
        assert_eq!(
            rows,
            Some(Some(42)),
            "estimated_rows must survive the mapping"
        );

        let definition = first.iter().find_map(|o| match o {
            SchemaObject::View { definition, .. } => Some(definition.clone()),
            _ => None,
        });
        assert_eq!(
            definition,
            Some("SELECT 1".to_string()),
            "view definition must survive the mapping"
        );
    }

    #[test]
    fn a_column_whose_parent_is_out_of_scope_is_dropped() {
        // Search-path scoping means a column can arrive without its relation.
        // Inventing a parent id (what this code used to do) produces an orphan
        // that no consumer can resolve; dropping it is correct.
        let connection = ConnectionId::new();
        let entries = vec![SchemaSnapshotEntry::Column {
            native_id: (99999u64 << 16) | 1,
            parent_schema: "other".to_string(),
            parent_table: "unseen".to_string(),
            name: "id".to_string(),
            data_type: "bigint".to_string(),
            nullable: true,
            ordinal: 1,
            default: None,
        }];
        assert!(SchemaService::objects_from_entries(connection, &entries).is_empty());
    }

    #[test]
    fn ids_differ_per_connection() {
        let entries = vec![table_entry(16384, "public", "users")];
        let a = SchemaService::objects_from_entries(ConnectionId::new(), &entries);
        let b = SchemaService::objects_from_entries(ConnectionId::new(), &entries);
        assert_ne!(a[0].id(), b[0].id(), "two databases may share OIDs");
    }
}
