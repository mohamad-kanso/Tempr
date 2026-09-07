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
    /// Schemas the connection can reference unqualified — its `search_path`
    /// plus `public`. Intended for catalog refreshes once the schema service
    /// adopts it: it matches what unqualified SQL can actually name, and keeps
    /// large multi-tenant databases from loading schemas nobody in this session
    /// will reference.
    SearchPath,
    /// Every non-system schema.
    All,
    Schema(String),
    Table {
        schema: String,
        table: String,
    },
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

    /// Cancel the currently executing query on this connection. Requires
    /// exclusive access, so it can only be called by whoever currently
    /// holds the connection — a concurrent caller should use
    /// `cancel_handle()` instead.
    async fn cancel(&mut self) -> Result<(), DriverError>;

    /// Whether the underlying transport is known to be dead (server closed
    /// the socket, connection task exited). Cheap, no I/O; used by the pool
    /// to evict broken connections before handing them out.
    fn is_closed(&self) -> bool;

    /// Obtain a cheap, cloneable handle that can cancel the query currently
    /// running on this connection from a *different* task, without needing
    /// exclusive (`&mut`) access — the connection may still be checked out
    /// and executing a query elsewhere.
    fn cancel_handle(&self) -> Box<dyn CancelHandle>;

    /// Perform a full schema introspection within the given scope.
    /// Returns structured metadata that SchemaService persists to cache.
    async fn snapshot_schema(
        &mut self,
        scope: SchemaScope,
    ) -> Result<Vec<SchemaSnapshotEntry>, DriverError>;

    /// Cheap change-detection sweep over `scope`: one row per relation and per
    /// column, carrying a version that moves when the object's definition
    /// changes. Callers diff two sweeps and re-introspect only what moved.
    ///
    /// Drivers that cannot do this return `DriverError::Unsupported`, and the
    /// caller falls back to a full introspection.
    async fn schema_fingerprints(
        &mut self,
        scope: SchemaScope,
    ) -> Result<Vec<SchemaFingerprint>, DriverError> {
        let _ = scope;
        Err(DriverError::Unsupported("schema_fingerprints".to_string()))
    }

    /// The engine's own keyword list, fetched once per schema refresh and
    /// cached with the catalog — never on the completion request path.
    /// Drivers with no such list return an empty vector.
    async fn keywords(&mut self) -> Result<Vec<String>, DriverError> {
        Ok(Vec::new())
    }
}

/// A handle capable of cancelling an in-flight query without exclusive
/// access to its `DriverConnection`. See `DriverConnection::cancel_handle`.
#[async_trait]
pub trait CancelHandle: Send + Sync {
    async fn cancel(&self) -> Result<(), DriverError>;
}

/// What a fingerprint refers to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ObjectKind {
    Relation,
    Column,
}

/// A cheap change marker for one schema object. `version` changes whenever the
/// object's definition changes; comparing two sweeps yields the set of objects
/// worth re-introspecting. PostgreSQL uses the catalog row's `xmin`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchemaFingerprint {
    pub native_id: u64,
    pub kind: ObjectKind,
    pub version: u64,
}

/// A single entry in a schema snapshot — flat list with implicit parent-child.
///
/// `native_id` is the database engine's own identifier for the object and must
/// be stable across refreshes and restarts: PostgreSQL uses `pg_class.oid` for
/// relations, `pg_proc.oid` for functions, and `(attrelid << 16) | attnum` for
/// columns. A driver whose engine has no stable identifier should hash the
/// object's qualified name into this field instead; the catalog then treats a
/// rename as a delete plus an insert, which is correct but coarser.
#[derive(Debug, Clone)]
pub enum SchemaSnapshotEntry {
    Table {
        native_id: u64,
        schema: String,
        name: String,
        estimated_rows: Option<u64>,
    },
    View {
        native_id: u64,
        schema: String,
        name: String,
        definition: String,
    },
    Column {
        native_id: u64,
        parent_schema: String,
        parent_table: String,
        name: String,
        data_type: String,
        nullable: bool,
        ordinal: usize,
        default: Option<String>,
    },
    Index {
        native_id: u64,
        parent_schema: String,
        parent_table: String,
        name: String,
        columns: Vec<String>,
        unique: bool,
        index_type: String,
    },
    Function {
        native_id: u64,
        schema: String,
        name: String,
        /// `(argument name, formatted type)`; unnamed arguments are `$1`, `$2`, …
        parameters: Vec<(String, String)>,
        return_type: String,
        language: String,
    },
}
