use crate::error::DriverError;
use tempr_domain::{Batch, ColumnSpec};

/// Trait object boundary for driver-specific streaming implementations.
#[async_trait::async_trait]
pub trait QueryStreamImpl: Send {
    /// Column metadata — available immediately after execute().
    fn columns(&self) -> &[ColumnSpec];

    /// Fetch the next batch of rows. Returns None when the result set
    /// is exhausted. Batch size is bounded to `batch_size`.
    async fn next_batch(&mut self) -> Result<Option<Batch>, DriverError>;

    /// Total rows affected (for DML). Valid only after stream exhausted.
    fn rows_affected(&self) -> u64;
}

/// Streaming, never buffer-all: rows arrive in bounded batches.
/// Dropping the stream cancels the server-side query.
pub struct QueryStream {
    inner: Box<dyn QueryStreamImpl>,
    batch_size: usize,
    finished: bool,
}

impl QueryStream {
    pub fn new(inner: Box<dyn QueryStreamImpl>, batch_size: usize) -> Self {
        Self {
            inner,
            batch_size,
            finished: false,
        }
    }

    pub fn columns(&self) -> &[ColumnSpec] {
        self.inner.columns()
    }

    pub async fn next_batch(&mut self) -> Result<Option<Batch>, DriverError> {
        if self.finished {
            return Ok(None);
        }
        let result = self.inner.next_batch().await?;
        if result.is_none() {
            self.finished = true;
        }
        Ok(result)
    }

    pub fn rows_affected(&self) -> u64 {
        self.inner.rows_affected()
    }

    pub fn is_finished(&self) -> bool {
        self.finished
    }

    pub fn batch_size(&self) -> usize {
        self.batch_size
    }
}
