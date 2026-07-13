#![deny(unsafe_code)]
#![cfg_attr(test, allow(clippy::expect_used, clippy::unwrap_used))]

pub mod registry;

pub use registry::{Service, ServiceError, ServiceRegistry};
