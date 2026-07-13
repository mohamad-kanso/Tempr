use std::pin::Pin;

use futures::StreamExt;
use tempr_db::{DriverError, QueryStreamImpl};
use tempr_domain::{Batch, ColumnSpec, Value};

pub(crate) struct PostgresStream {
    columns: Vec<ColumnSpec>,
    stream: Option<Pin<Box<tokio_postgres::RowStream>>>,
    batch_size: usize,
    rows_affected: u64,
}

impl PostgresStream {
    pub fn new(
        columns: Vec<ColumnSpec>,
        stream: tokio_postgres::RowStream,
        batch_size: usize,
    ) -> Self {
        Self {
            columns,
            stream: Some(Box::pin(stream)),
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

fn decode_pg_value(col: &tokio_postgres::Column, raw: Option<&str>) -> Value {
    use crate::decode::decode_value;
    match raw {
        Some(text) => decode_value(col.type_(), Some(text)).unwrap_or_else(|e| Value::Custom {
            type_name: format!("decode_error: {e}"),
            raw_bytes: Vec::new(),
        }),
        None => Value::Null,
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

        let mut rows = Vec::with_capacity(self.batch_size);

        for _ in 0..self.batch_size {
            match stream.next().await {
                Some(Ok(row)) => {
                    let mut values = Vec::with_capacity(row.len());
                    for (i, col) in row.columns().iter().enumerate() {
                        let raw: Option<String> = row.try_get(i).ok().flatten();
                        values.push(decode_pg_value(col, raw.as_deref()));
                    }
                    rows.push(values);
                }
                Some(Err(e)) => return Err(DriverError::Query(e.to_string())),
                None => break,
            }
        }

        if rows.is_empty() {
            Ok(None)
        } else {
            let batch_index = self.rows_affected as usize / self.batch_size;
            self.rows_affected += rows.len() as u64;
            Ok(Some(Batch { rows, batch_index }))
        }
    }

    fn rows_affected(&self) -> u64 {
        self.rows_affected
    }
}
