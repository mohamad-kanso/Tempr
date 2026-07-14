#![deny(unsafe_code)]
#![cfg_attr(test, allow(clippy::expect_used, clippy::unwrap_used))]

pub mod driver;
pub mod error;
pub mod stream;

pub use driver::{
    CancelHandle, DatabaseDriver, DriverConnection, EngineId, SchemaScope, SchemaSnapshotEntry,
};
pub use error::DriverError;
pub use stream::{QueryStream, QueryStreamImpl};

#[cfg(test)]
mod tests {
    use crate::error::DriverError;
    use crate::stream::{QueryStream, QueryStreamImpl};
    use tempr_domain::{Batch, ColumnSpec, Value, ValueType};

    /// Mock streaming implementation for testing.
    struct MockStream {
        columns: Vec<ColumnSpec>,
        batches: Vec<Batch>,
        index: usize,
        rows_affected: u64,
    }

    impl MockStream {
        fn empty() -> Self {
            Self {
                columns: Vec::new(),
                batches: Vec::new(),
                index: 0,
                rows_affected: 0,
            }
        }

        fn with_batches(batches: Vec<Batch>) -> Self {
            let columns = vec![ColumnSpec {
                name: "col1".to_string(),
                ordinal: 0,
                data_type: "text".to_string(),
                value_type: ValueType::String,
                nullable: true,
                table_schema: None,
                table_name: None,
            }];
            Self {
                columns,
                batches,
                index: 0,
                rows_affected: 0,
            }
        }

        fn with_affected(rows_affected: u64) -> Self {
            Self {
                columns: Vec::new(),
                batches: Vec::new(),
                index: 0,
                rows_affected,
            }
        }
    }

    #[async_trait::async_trait]
    impl QueryStreamImpl for MockStream {
        fn columns(&self) -> &[ColumnSpec] {
            &self.columns
        }

        async fn next_batch(&mut self) -> Result<Option<Batch>, DriverError> {
            if self.index < self.batches.len() {
                let batch = self.batches[self.index].clone();
                self.index += 1;
                Ok(Some(batch))
            } else {
                Ok(None)
            }
        }

        fn rows_affected(&self) -> u64 {
            self.rows_affected
        }
    }

    #[tokio::test]
    async fn empty_stream_returns_none_immediately() {
        let mut stream = QueryStream::new(Box::new(MockStream::empty()), 100);
        assert!(!stream.is_finished());
        let batch = stream.next_batch().await.unwrap();
        assert!(batch.is_none());
        assert!(stream.is_finished());
    }

    #[tokio::test]
    async fn empty_stream_returns_none_on_subsequent_calls() {
        let mut stream = QueryStream::new(Box::new(MockStream::empty()), 100);
        let _ = stream.next_batch().await.unwrap();
        assert!(stream.is_finished());
        // Should short-circuit
        let batch = stream.next_batch().await.unwrap();
        assert!(batch.is_none());
    }

    #[tokio::test]
    async fn single_batch_stream() {
        let batch = Batch {
            rows: vec![vec![Value::Text("hello".to_string())]],
            batch_index: 0,
        };
        let mut stream = QueryStream::new(Box::new(MockStream::with_batches(vec![batch])), 100);

        let b1 = stream.next_batch().await.unwrap();
        assert!(b1.is_some());
        assert_eq!(b1.unwrap().rows.len(), 1);

        let b2 = stream.next_batch().await.unwrap();
        assert!(b2.is_none());
        assert!(stream.is_finished());
    }

    #[tokio::test]
    async fn multiple_batches() {
        let batches = vec![
            Batch {
                rows: vec![vec![Value::Int8(1)]],
                batch_index: 0,
            },
            Batch {
                rows: vec![vec![Value::Int8(2)]],
                batch_index: 1,
            },
            Batch {
                rows: vec![vec![Value::Int8(3)]],
                batch_index: 2,
            },
        ];
        let mut stream = QueryStream::new(Box::new(MockStream::with_batches(batches)), 100);

        let mut count = 0;
        while let Some(batch) = stream.next_batch().await.unwrap() {
            count += batch.rows.len();
        }
        assert_eq!(count, 3);
        assert!(stream.is_finished());
    }

    #[tokio::test]
    async fn columns_returned_immediately() {
        let batch = Batch {
            rows: vec![],
            batch_index: 0,
        };
        let stream = QueryStream::new(Box::new(MockStream::with_batches(vec![batch])), 100);
        assert_eq!(stream.columns().len(), 1);
        assert_eq!(stream.columns()[0].name, "col1");
    }

    #[tokio::test]
    async fn rows_affected_delegates() {
        let stream = QueryStream::new(Box::new(MockStream::with_affected(42)), 100);
        assert_eq!(stream.rows_affected(), 42);
    }

    #[tokio::test]
    async fn batch_size_reported() {
        let stream = QueryStream::new(Box::new(MockStream::empty()), 4000);
        assert_eq!(stream.batch_size(), 4000);
    }
}
