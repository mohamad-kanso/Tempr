use crate::ids::{ConnectionId, SchemaObjectId, SchemaSnapshotId};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchemaSnapshot {
    pub id: SchemaSnapshotId,
    pub connection_id: ConnectionId,
    pub version: u64,
    pub fetched_at: DateTime<Utc>,
    pub objects: Vec<SchemaObject>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SchemaObject {
    Table {
        id: SchemaObjectId,
        schema: String,
        name: String,
        estimated_rows: Option<u64>,
    },
    View {
        id: SchemaObjectId,
        schema: String,
        name: String,
        definition: String,
    },
    Column {
        id: SchemaObjectId,
        parent_id: SchemaObjectId,
        name: String,
        data_type: String,
        nullable: bool,
        ordinal: usize,
        default: Option<String>,
    },
    Index {
        id: SchemaObjectId,
        parent_table_id: SchemaObjectId,
        name: String,
        columns: Vec<String>,
        unique: bool,
        index_type: String,
    },
    Function {
        id: SchemaObjectId,
        schema: String,
        name: String,
        parameters: Vec<(String, String)>,
        return_type: String,
        language: String,
    },
}

impl SchemaObject {
    pub fn id(&self) -> SchemaObjectId {
        match self {
            SchemaObject::Table { id, .. }
            | SchemaObject::View { id, .. }
            | SchemaObject::Column { id, .. }
            | SchemaObject::Index { id, .. }
            | SchemaObject::Function { id, .. } => *id,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::{ConnectionId, SchemaObjectId, SchemaSnapshotId};

    fn make_snapshot(objects: Vec<SchemaObject>) -> SchemaSnapshot {
        SchemaSnapshot {
            id: SchemaSnapshotId::new(),
            connection_id: ConnectionId::new(),
            version: 1,
            fetched_at: Utc::now(),
            objects,
        }
    }

    #[test]
    fn schema_object_id_accessor() {
        let id = SchemaObjectId::new();
        let obj = SchemaObject::Table {
            id,
            schema: "public".to_string(),
            name: "users".to_string(),
            estimated_rows: Some(1000),
        };
        assert_eq!(obj.id(), id);
    }

    #[test]
    fn snapshot_serde_roundtrip() {
        let snapshot = make_snapshot(vec![
            SchemaObject::Table {
                id: SchemaObjectId::new(),
                schema: "public".to_string(),
                name: "users".to_string(),
                estimated_rows: None,
            },
            SchemaObject::Column {
                id: SchemaObjectId::new(),
                parent_id: SchemaObjectId::new(),
                name: "id".to_string(),
                data_type: "int8".to_string(),
                nullable: false,
                ordinal: 0,
                default: None,
            },
        ]);
        let json = serde_json::to_string(&snapshot).expect("serialize");
        let back: SchemaSnapshot = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(snapshot.id, back.id);
        assert_eq!(snapshot.version, back.version);
        assert_eq!(snapshot.objects.len(), back.objects.len());
    }
}
