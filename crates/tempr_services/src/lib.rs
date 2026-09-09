#![deny(unsafe_code)]
#![cfg_attr(test, allow(clippy::expect_used, clippy::unwrap_used))]

pub mod command;
pub mod connection;
pub mod fuzzy;
pub mod query;
pub mod registry;
pub mod schema;

pub use command::{
    CommandContribution, CommandMatch, CommandMeta, CommandService, KeybindingOverrides,
};
pub use connection::{ConnectionService, PoolConfig, PooledConnection};
pub use fuzzy::{FuzzyMatch, fuzzy_match};
pub use query::{QueryService, RowSink};
pub use registry::{Service, ServiceError, ServiceRegistry};
pub use schema::{FullRefreshReason, RefreshPath, SchemaService};
