use std::pin::Pin;

use futures::StreamExt;
use tempr_db::{DriverError, QueryStreamImpl};
use tempr_domain::{Batch, ColumnSpec, Value};

type BatchStream = Pin<Box<dyn futures::Stream<Item = Result<Batch, DriverError>> + Send>>;

pub(crate) struct PostgresStream {
    columns: Vec<ColumnSpec>,
    stream: Option<BatchStream>,
    batch_size: usize,
    rows_affected: u64,
}

impl PostgresStream {
    pub fn from_rows(columns: Vec<ColumnSpec>, rows: Vec<Vec<Value>>, batch_size: usize) -> Self {
        use futures::stream;

        let batches: Vec<Result<Batch, DriverError>> = rows
            .into_iter()
            .enumerate()
            .map(|(i, row)| {
                Ok(Batch {
                    rows: vec![row],
                    batch_index: i,
                })
            })
            .collect();

        Self {
            columns,
            stream: Some(Box::pin(stream::iter(batches))),
            batch_size,
            rows_affected: 0,
        }
    }

    pub fn for_dml(rows_affected: u64) -> Self {
        Self {
            columns: Vec::new(),
            stream: None,
            batch_size: 0,
            rows_affected,
        }
    }
}

#[async_trait::async_trait]
impl QueryStreamImpl for PostgresStream {
    fn columns(&self) -> &[ColumnSpec] {
        &self.columns
    }

    async fn next_batch(&mut self) -> Result<Option<Batch>, DriverError> {
        let mut stream = match self.stream.as_mut() {
            Some(s) => s.as_mut(),
            None => return Ok(None),
        };

        match stream.next().await {
            Some(Ok(mut batch)) => {
                self.rows_affected += batch.rows.len() as u64;
                batch.batch_index =
                    (self.rows_affected as usize / self.batch_size).saturating_sub(1);
                Ok(Some(batch))
            }
            Some(Err(e)) => Err(e),
            None => Ok(None),
        }
    }

    fn rows_affected(&self) -> u64 {
        self.rows_affected
    }
}
