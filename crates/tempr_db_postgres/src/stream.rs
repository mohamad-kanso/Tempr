use tempr_db::{DriverError, QueryStreamImpl};
use tempr_domain::{Batch, ColumnSpec, Value};
use tokio_postgres::Row;

use crate::decode::decode_value;

/// Wraps an already-fetched `Vec<tokio_postgres::Row>` and yields
/// `batch_size`-sized decoded batches on successive `next_batch()` calls.
///
/// True lazy wire streaming (`query_raw`) is deferred to a follow-up —
/// see docs/TODO.md.  The current `client.query()` path materialises all
/// rows first, which is correct but uses more memory for large result sets.
pub(crate) struct PostgresStream {
    columns: Vec<ColumnSpec>,
    rows: Vec<Row>,
    offset: usize,
    batch_size: usize,
    rows_affected: u64,
    next_batch_index: usize,
}

fn decode_row(row: &Row) -> Vec<Value> {
    row.columns()
        .iter()
        .enumerate()
        .map(|(i, col)| {
            decode_value(row, i, col.type_()).unwrap_or_else(|e| Value::Custom {
                type_name: format!("decode_error: {e}"),
                raw_bytes: Vec::new(),
            })
        })
        .collect()
}

impl PostgresStream {
    pub fn from_rows(columns: Vec<ColumnSpec>, rows: Vec<Row>, batch_size: usize) -> Self {
        Self {
            columns,
            rows,
            offset: 0,
            batch_size,
            rows_affected: 0,
            next_batch_index: 0,
        }
    }

    pub fn for_dml(rows_affected: u64) -> Self {
        Self {
            columns: Vec::new(),
            rows: Vec::new(),
            offset: 0,
            batch_size: 0,
            rows_affected,
            next_batch_index: 0,
        }
    }
}

#[async_trait::async_trait]
impl QueryStreamImpl for PostgresStream {
    fn columns(&self) -> &[ColumnSpec] {
        &self.columns
    }

    async fn next_batch(&mut self) -> Result<Option<Batch>, DriverError> {
        if self.offset >= self.rows.len() {
            return Ok(None);
        }

        let end = (self.offset + self.batch_size).min(self.rows.len());
        let chunk: Vec<Vec<Value>> = self.rows[self.offset..end].iter().map(decode_row).collect();
        let count = chunk.len();
        self.offset = end;
        self.rows_affected += count as u64;

        let batch = Batch {
            rows: chunk,
            batch_index: self.next_batch_index,
        };
        self.next_batch_index += 1;
        Ok(Some(batch))
    }

    fn rows_affected(&self) -> u64 {
        self.rows_affected
    }
}
