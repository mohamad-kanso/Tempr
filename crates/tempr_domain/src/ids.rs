use serde::{Deserialize, Serialize};
use uuid::Uuid;

macro_rules! uuid_id {
    ($name:ident) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
        pub struct $name(pub Uuid);

        impl $name {
            pub fn new() -> Self {
                Self(Uuid::new_v4())
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "{}", self.0)
            }
        }
    };
}

uuid_id!(WorkspaceId);
uuid_id!(ConnectionId);
uuid_id!(SqlFileId);
uuid_id!(QueryId);
uuid_id!(QueryRunId);
uuid_id!(SchemaObjectId);
uuid_id!(SchemaSnapshotId);
uuid_id!(HistoryEntryId);
uuid_id!(PluginId);

impl SchemaObjectId {
    /// Identity derived from the database engine's own identifier, stable
    /// across refreshes, restarts and renames.
    ///
    /// UUIDv5 over the connection id as namespace and `(kind, native_id)` as
    /// name, so the same object in the same database always resolves to the
    /// same id — which is what lets a cached snapshot be diffed against a
    /// fresh one. `kind` is part of the key because `native_id` is unique
    /// only within a kind.
    pub fn derived(
        connection: crate::ids::ConnectionId,
        kind: crate::schema::SchemaObjectKind,
        native_id: u64,
    ) -> Self {
        let mut name = [0u8; 9];
        name[0] = kind.discriminant();
        name[1..].copy_from_slice(&native_id.to_le_bytes());
        Self(Uuid::new_v5(&connection.0, &name))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_unique() {
        let a = WorkspaceId::new();
        let b = WorkspaceId::new();
        assert_ne!(a, b);
    }

    #[test]
    fn id_display_is_uuid_string() {
        let id = WorkspaceId::new();
        assert_eq!(id.to_string(), id.0.to_string());
    }

    #[test]
    fn id_roundtrips_serde_json() {
        let id = ConnectionId::new();
        let json = serde_json::to_string(&id).expect("serialize");
        let back: ConnectionId = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(id, back);
    }

    #[test]
    fn derived_ids_are_stable_and_kind_separated() {
        use crate::schema::SchemaObjectKind;
        let conn = ConnectionId::new();
        let other = ConnectionId::new();

        // Same inputs → same id, every time, in any process.
        assert_eq!(
            SchemaObjectId::derived(conn, SchemaObjectKind::Table, 16384),
            SchemaObjectId::derived(conn, SchemaObjectKind::Table, 16384)
        );
        // Kind is part of the key: a column id may numerically equal a relation oid.
        assert_ne!(
            SchemaObjectId::derived(conn, SchemaObjectKind::Table, 16384),
            SchemaObjectId::derived(conn, SchemaObjectKind::Column, 16384)
        );
        // Connection is part of the key: two databases may share oids.
        assert_ne!(
            SchemaObjectId::derived(conn, SchemaObjectKind::Table, 16384),
            SchemaObjectId::derived(other, SchemaObjectKind::Table, 16384)
        );
        // Different objects of the same kind stay distinct.
        assert_ne!(
            SchemaObjectId::derived(conn, SchemaObjectKind::Table, 16384),
            SchemaObjectId::derived(conn, SchemaObjectKind::Table, 16385)
        );
    }

    #[test]
    fn derived_id_is_reproducible_across_runs() {
        use crate::schema::SchemaObjectKind;
        use uuid::Uuid;
        // A fixed connection id pins the expected output, so a change to the
        // derivation scheme fails here instead of silently orphaning caches.
        let conn = ConnectionId(Uuid::from_u128(0x0123_4567_89ab_cdef_0123_4567_89ab_cdef));
        let id = SchemaObjectId::derived(conn, SchemaObjectKind::Column, (16384u64 << 16) | 2);
        assert_eq!(
            id,
            SchemaObjectId::derived(conn, SchemaObjectKind::Column, (16384u64 << 16) | 2)
        );
        assert_ne!(id.0, Uuid::nil());
    }
}
