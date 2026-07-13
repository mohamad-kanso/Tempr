#![deny(unsafe_code)]
#![cfg_attr(test, allow(clippy::expect_used, clippy::unwrap_used))]

pub mod driver;
pub mod error;
pub mod stream;

pub use driver::{DatabaseDriver, DriverConnection, EngineId, SchemaScope, SchemaSnapshotEntry};
pub use error::DriverError;
pub use stream::{QueryStream, QueryStreamImpl};
