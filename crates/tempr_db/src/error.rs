use thiserror::Error;

#[derive(Debug, Error)]
pub enum DriverError {
    #[error("connection refused: {0}")]
    ConnectionRefused(String),

    #[error("authentication failed: {0}")]
    AuthFailed(String),

    #[error("query error: {0}")]
    Query(String),

    #[error("cancelled")]
    Cancelled,

    #[error("timeout")]
    Timeout,

    #[error("engine not found: {0}")]
    EngineNotFound(String),

    #[error("internal: {0}")]
    Internal(String),
}
