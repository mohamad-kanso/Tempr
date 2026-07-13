use thiserror::Error;

#[derive(Debug, Error)]
pub enum WorkspaceError {
    #[error("workspace not found at path: {path}")]
    NotFound { path: String },

    #[error("workspace manifest is corrupted or malformed: {reason}")]
    Corrupted { reason: String },

    #[error("workspace migration failed from v{from} to v{to}: {reason}")]
    MigrationFailed { from: u32, to: u32, reason: String },

    #[error("workspace is already open")]
    AlreadyOpen,

    #[error("permission denied accessing workspace at {path}: {reason}")]
    PermissionDenied { path: String, reason: String },

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}
