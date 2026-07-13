#![deny(unsafe_code)]
#![cfg_attr(test, allow(clippy::expect_used, clippy::unwrap_used))]

pub mod error;
pub mod manifest;
pub mod storage;

pub use error::WorkspaceError;
pub use manifest::{CURRENT_FORMAT_VERSION, ConnectionConfig, WorkspaceInfo, WorkspaceManifest};
pub use storage::{FileSystemStorage, Storage};
