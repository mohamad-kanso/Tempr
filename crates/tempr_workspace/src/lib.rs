#![deny(unsafe_code)]
#![cfg_attr(test, allow(clippy::expect_used, clippy::unwrap_used))]

pub mod catalog;
pub mod error;
pub mod manifest;
pub mod settings;
pub mod storage;

pub use catalog::{
    CATALOG_FORMAT_VERSION, CATALOG_MAGIC, content_hash, decode_catalog, encode_catalog,
};
pub use error::WorkspaceError;
pub use manifest::{
    CURRENT_FORMAT_VERSION, ConnectionConfig, WorkspaceInfo, WorkspaceManifest, load_manifest_from,
    parse_manifest,
};
pub use settings::{
    UserSettings, load_user_settings, load_user_settings_from, parse_user_settings,
    user_settings_path,
};
pub use storage::{CatalogCacheFile, FileCatalogCache, FileSystemStorage, Storage};
