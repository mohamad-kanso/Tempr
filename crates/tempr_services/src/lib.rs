#![deny(unsafe_code)]
#![cfg_attr(test, allow(clippy::expect_used, clippy::unwrap_used))]

pub mod connection;
pub mod query;
pub mod registry;
pub mod schema;

pub use connection::ConnectionService;
pub use query::QueryService;
pub use registry::{Service, ServiceError, ServiceRegistry};
pub use schema::SchemaService;
