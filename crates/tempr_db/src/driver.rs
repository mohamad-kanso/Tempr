use async_trait::async_trait;

use crate::error::DriverError;
use crate::stream::QueryStream;
use tempr_domain::{Connection, Value};

/// Unique engine identifier, used as the key in driver registration.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct EngineId(pub String);

/// Scope for schema introspection.
#[derive(Debug, Clone)]
pub enum SchemaScope {
    All,
    Schema(String),
    Table { schema: String, table: String },
}

/// The root trait that every database engine plugin implements.
/// Intentionally thin — engine identity and connection creation.
#[async_trait]
pub trait DatabaseDriver: Send + Sync {
    /// Engine identifier, e.g. "postgresql", "mysql", "sqlite".
    fn engine(&self) -> EngineId;

    /// Establish a new connection to the database.
    async fn connect(
        &self,
        connection: &Connection,
    ) -> Result<Box<dyn DriverConnection>, DriverError>;
}

/// The active connection handle returned by `DatabaseDriver::connect`.
/// Every method is cancellable (the async task can be dropped to abort).
#[async_trait]
pub trait DriverConnection: Send + Sync {
    /// Execute a SQL statement and return a streaming result handle.
    /// For DDL/DML that returns no rows, the stream yields zero batches
    /// and reports the affected row count via `QueryStream::rows_affected()`.
    async fn execute(&mut self, sql: &str, params: &[Value]) -> Result<QueryStream, DriverError>;

    /// Cancel the currently executing query on this connection.
    async fn cancel(&mut self) -> Result<(), DriverError>;

    /// Perform a full schema introspection within the given scope.
    /// Returns structured metadata that SchemaService persists to cache.
    async fn snapshot_schema(
        &mut self,
        scope: SchemaScope,
    ) -> Result<Vec<SchemaSnapshotEntry>, DriverError>;
}

/// A single entry in a schema snapshot — flat list with implicit parent-child.
#[derive(Debug, Clone)]
pub enum SchemaSnapshotEntry {
    Table {
        schema: String,
        name: String,
        estimated_rows: Option<u64>,
    },
    View {
        schema: String,
        name: String,
        definition: String,
    },
    Column {
        parent_schema: String,
        parent_table: String,
        name: String,
        data_type: String,
        nullable: bool,
        ordinal: usize,
        default: Option<String>,
    },
    Index {
        parent_schema: String,
        parent_table: String,
        name: String,
        columns: Vec<String>,
        unique: bool,
        index_type: String,
    },
}
