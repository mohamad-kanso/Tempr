#![deny(unsafe_code)]
#![cfg_attr(test, allow(clippy::expect_used, clippy::unwrap_used))]

pub mod command;
pub mod connection;
pub mod intelligence;
pub mod query;
pub mod registry;
pub mod schema;

pub use command::{
    CommandContribution, CommandMatch, CommandMeta, CommandService, KeybindingOverrides,
};
pub use connection::{ConnectionService, PoolConfig, PooledConnection};
pub use intelligence::IntelligenceService;
pub use query::{QueryService, RowSink};
pub use registry::{Service, ServiceError, ServiceRegistry};
pub use schema::{FullRefreshReason, RefreshPath, SchemaService};
pub use tempr_domain::text::{FuzzyMatch, fuzzy_match};
