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
    fn derived_id_matches_pinned_golden_values() {
        use crate::schema::SchemaObjectKind;
        use uuid::Uuid;
        // The derivation is a persisted wire format: every `SchemaObjectId`
        // ever written to a cache file on disk was computed by this exact
        // byte layout. These are not "does it equal itself" checks — they
        // are hardcoded UUIDs computed once from the current implementation
        // and pinned here. If either assertion fails, the derivation scheme
        // changed (e.g. `to_le_bytes()` became `to_ne_bytes()`, or the
        // discriminant byte moved), and EVERY existing cache file is now
        // silently orphaned — its ids no longer match what a fresh schema
        // fetch would derive. `CATALOG_FORMAT_VERSION` must be bumped in the
        // same change that updates these golden values.
        //
        // Byte layout encoded by both golden values: the UUIDv5 name is 9
        // bytes, `[discriminant: u8][native_id: u64 little-endian]`, hashed
        // with the connection id as the namespace.
        let conn = ConnectionId(Uuid::from_u128(0x0123_4567_89ab_cdef_0123_4567_89ab_cdef));
        let native_id = (16384u64 << 16) | 2;

        // kind = Column (discriminant 2). Name bytes:
        // 02 02 00 00 00 00 00 10 00
        let column_id = SchemaObjectId::derived(conn, SchemaObjectKind::Column, native_id);
        assert_eq!(
            column_id.0,
            Uuid::parse_str("cdd409ec-1e98-573b-b875-da6d03ad753b").unwrap()
        );

        // Same native_id, kind = Table (discriminant 0) instead of Column.
        // Name bytes: 00 02 00 00 00 00 00 10 00
        // Pinning this alongside the Column value catches a reordering of
        // the name bytes (e.g. discriminant moved after native_id instead
        // of before it), which would otherwise leave the Column-only
        // assertion unable to detect it.
        let table_id = SchemaObjectId::derived(conn, SchemaObjectKind::Table, native_id);
        assert_eq!(
            table_id.0,
            Uuid::parse_str("c3150d09-2dd9-5ccc-9999-43300201c946").unwrap()
        );
    }
}
