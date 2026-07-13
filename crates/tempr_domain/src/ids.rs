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
}
