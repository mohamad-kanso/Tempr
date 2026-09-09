use crate::ids::{ConnectionId, SchemaObjectId, SchemaSnapshotId};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// What a schema object is. Part of every object's identity: `native_id` is
/// unique only within a kind (a packed column id can numerically equal a
/// relation OID), so consumers key on the pair.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SchemaObjectKind {
    Table,
    View,
    Column,
    Index,
    Function,
}

impl SchemaObjectKind {
    /// Stable discriminant used in identity derivation. Never renumber these:
    /// a change orphans every cache file in the wild.
    pub fn discriminant(self) -> u8 {
        match self {
            SchemaObjectKind::Table => 0,
            SchemaObjectKind::View => 1,
            SchemaObjectKind::Column => 2,
            SchemaObjectKind::Index => 3,
            SchemaObjectKind::Function => 4,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchemaSnapshot {
    pub id: SchemaSnapshotId,
    pub connection_id: ConnectionId,
    pub version: u64,
    pub fetched_at: DateTime<Utc>,
    pub objects: Vec<SchemaObject>,
    /// The engine's keyword list, fetched with the snapshot so completion
    /// never queries the database (docs/12-sql-intelligence.md).
    #[serde(default)]
    pub keywords: Vec<String>,
    /// Change markers for every object in scope at the time of the snapshot.
    /// An incremental refresh diffs a fresh sweep against these.
    #[serde(default)]
    pub fingerprints: Vec<SchemaFingerprintRecord>,
}

/// One object's change marker, as reported by the driver's fingerprint sweep
/// and persisted with the snapshot so a later sweep can be diffed against it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SchemaFingerprintRecord {
    pub kind: SchemaObjectKind,
    pub native_id: u64,
    pub version: u64,
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

    pub fn kind(&self) -> SchemaObjectKind {
        match self {
            SchemaObject::Table { .. } => SchemaObjectKind::Table,
            SchemaObject::View { .. } => SchemaObjectKind::View,
            SchemaObject::Column { .. } => SchemaObjectKind::Column,
            SchemaObject::Index { .. } => SchemaObjectKind::Index,
            SchemaObject::Function { .. } => SchemaObjectKind::Function,
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
            keywords: Vec::new(),
            fingerprints: Vec::new(),
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
    fn schema_object_reports_its_kind() {
        let obj = SchemaObject::Column {
            id: SchemaObjectId::new(),
            parent_id: SchemaObjectId::new(),
            name: "id".to_string(),
            data_type: "int8".to_string(),
            nullable: false,
            ordinal: 1,
            default: None,
        };
        assert_eq!(obj.kind(), SchemaObjectKind::Column);
    }

    #[test]
    fn snapshot_serde_carries_keywords_and_fingerprints() {
        let mut snapshot = make_snapshot(vec![]);
        snapshot.keywords = vec!["select".to_string(), "join".to_string()];
        snapshot.fingerprints = vec![SchemaFingerprintRecord {
            kind: SchemaObjectKind::Table,
            native_id: 16384,
            version: 42,
        }];

        let json = serde_json::to_string(&snapshot).expect("serialize");
        let back: SchemaSnapshot = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.keywords, snapshot.keywords);
        assert_eq!(back.fingerprints.len(), 1);
        assert_eq!(back.fingerprints[0].native_id, 16384);
        assert_eq!(back.fingerprints[0].kind, SchemaObjectKind::Table);
    }

    #[test]
    fn older_snapshots_without_the_new_fields_still_load() {
        // A snapshot serialized before these fields existed must not fail to
        // parse — the cache would otherwise be discarded on every upgrade.
        let json = r#"{
            "id": "00000000-0000-0000-0000-000000000001",
            "connection_id": "00000000-0000-0000-0000-000000000002",
            "version": 3,
            "fetched_at": "2026-09-08T00:00:00Z",
            "objects": []
        }"#;
        let back: SchemaSnapshot = serde_json::from_str(json).expect("deserialize");
        assert!(back.keywords.is_empty());
        assert!(back.fingerprints.is_empty());
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
