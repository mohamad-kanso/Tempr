#![deny(unsafe_code)]
#![cfg_attr(test, allow(clippy::expect_used, clippy::unwrap_used))]

pub mod connection;
pub mod history;
pub mod ids;
pub mod query;
pub mod schema;
pub mod workspace;

pub use connection::{Connection, ConnectionState, DriverKind, SecretRef};
pub use history::HistoryEntry;
pub use ids::{
    ConnectionId, HistoryEntryId, PluginId, QueryId, QueryRunId, SchemaObjectId, SchemaSnapshotId,
    SqlFileId, WorkspaceId,
};
pub use query::{ColumnMeta, Query, QueryOutcome, QueryRun, ResultSet, Value};
pub use schema::{SchemaObject, SchemaSnapshot};
pub use workspace::{SqlFile, Workspace, WorkspaceSettings};
