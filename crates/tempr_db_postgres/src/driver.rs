use tempr_db::stream::QueryStream;
use tempr_db::{
    DatabaseDriver, DriverConnection, DriverError, EngineId, SchemaScope, SchemaSnapshotEntry,
};
use tempr_domain::{ColumnSpec, Connection, Value};
use tokio_postgres::NoTls;

use crate::stream::PostgresStream;

const DEFAULT_BATCH_SIZE: usize = 4000;

pub struct PostgresDriver;

impl Default for PostgresDriver {
    fn default() -> Self {
        Self::new()
    }
}

impl PostgresDriver {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait::async_trait]
impl DatabaseDriver for PostgresDriver {
    fn engine(&self) -> EngineId {
        EngineId("postgresql".to_string())
    }

    async fn connect(
        &self,
        connection: &Connection,
    ) -> Result<Box<dyn DriverConnection>, DriverError> {
        let host = &connection.host;
        let port = connection.port;
        let dbname = &connection.database;
        let user = &connection.username;

        let conn_str =
            format!("host={host} port={port} dbname={dbname} user={user} sslmode=require");

        let (client, connection_handle) =
            tokio_postgres::connect(&conn_str, NoTls)
                .await
                .map_err(|e| {
                    if e.to_string().contains("password authentication failed")
                        || e.to_string().contains("Authentication failure")
                    {
                        DriverError::AuthFailed(e.to_string())
                    } else {
                        DriverError::ConnectionRefused(e.to_string())
                    }
                })?;

        tokio::spawn(async move {
            if let Err(e) = connection_handle.await {
                tracing::error!("PostgreSQL connection task error: {e}");
            }
        });

        Ok(Box::new(PostgresConnection {
            client,
            batch_size: DEFAULT_BATCH_SIZE,
        }))
    }
}

struct PostgresConnection {
    client: tokio_postgres::Client,
    batch_size: usize,
}

#[async_trait::async_trait]
impl DriverConnection for PostgresConnection {
    async fn execute(&mut self, sql: &str, _params: &[Value]) -> Result<QueryStream, DriverError> {
        let trimmed = sql.trim().to_uppercase();
        let is_dml = trimmed.starts_with("INSERT")
            || trimmed.starts_with("UPDATE")
            || trimmed.starts_with("DELETE")
            || trimmed.starts_with("CREATE")
            || trimmed.starts_with("ALTER")
            || trimmed.starts_with("DROP")
            || trimmed.starts_with("TRUNCATE");

        if is_dml {
            let rows_affected = self
                .client
                .execute(sql, &[])
                .await
                .map_err(|e| DriverError::Query(e.to_string()))?;
            return Ok(QueryStream::new(
                Box::new(PostgresStream::for_dml(rows_affected)),
                self.batch_size,
            ));
        }

        let stmt = self
            .client
            .prepare(sql)
            .await
            .map_err(|e| DriverError::Query(e.to_string()))?;

        let columns: Vec<ColumnSpec> = stmt
            .columns()
            .iter()
            .enumerate()
            .map(|(i, col)| ColumnSpec {
                name: col.name().to_string(),
                ordinal: i,
                data_type: col.type_().name().to_string(),
                value_type: crate::pg_type_to_value_type(col.type_()),
                nullable: true,
                table_schema: None,
                table_name: None,
            })
            .collect();

        let stream = self
            .client
            .query_raw(&stmt, &[] as &[&str])
            .await
            .map_err(|e| DriverError::Query(e.to_string()))?;

        Ok(QueryStream::new(
            Box::new(PostgresStream::new(columns, stream, self.batch_size)),
            self.batch_size,
        ))
    }

    async fn cancel(&mut self) -> Result<(), DriverError> {
        self.client
            .cancel_token()
            .cancel_query(NoTls)
            .await
            .map_err(|e| DriverError::Internal(format!("cancel failed: {e}")))?;
        Ok(())
    }

    async fn snapshot_schema(
        &mut self,
        scope: SchemaScope,
    ) -> Result<Vec<SchemaSnapshotEntry>, DriverError> {
        let mut entries = Vec::new();

        // Tables — use parameterized queries to prevent SQL injection
        match &scope {
            SchemaScope::All => {
                if let Ok(rows) = self
                    .client
                    .query(
                        "SELECT schemaname, tablename FROM pg_tables \
                         WHERE schemaname NOT IN ('pg_catalog', 'information_schema')",
                        &[],
                    )
                    .await
                {
                    for row in rows {
                        entries.push(SchemaSnapshotEntry::Table {
                            schema: row.get(0),
                            name: row.get(1),
                            estimated_rows: None,
                        });
                    }
                }
            }
            SchemaScope::Schema(s) => {
                if let Ok(rows) = self
                    .client
                    .query(
                        "SELECT schemaname, tablename FROM pg_tables WHERE schemaname = $1",
                        &[s],
                    )
                    .await
                {
                    for row in rows {
                        entries.push(SchemaSnapshotEntry::Table {
                            schema: row.get(0),
                            name: row.get(1),
                            estimated_rows: None,
                        });
                    }
                }
            }
            SchemaScope::Table { schema, table } => {
                if let Ok(rows) = self
                    .client
                    .query(
                        "SELECT schemaname, tablename FROM pg_tables \
                         WHERE schemaname = $1 AND tablename = $2",
                        &[schema, table],
                    )
                    .await
                {
                    for row in rows {
                        entries.push(SchemaSnapshotEntry::Table {
                            schema: row.get(0),
                            name: row.get(1),
                            estimated_rows: None,
                        });
                    }
                }
            }
        }

        // Columns
        let col_query = "SELECT table_schema, table_name, column_name, data_type, \
                         is_nullable, ordinal_position, column_default \
                         FROM information_schema.columns \
                         WHERE table_schema NOT IN ('pg_catalog', 'information_schema') \
                         ORDER BY table_schema, table_name, ordinal_position";

        if let Ok(rows) = self.client.query(col_query, &[]).await {
            for row in rows {
                let nullable_str: Option<String> = row.get(4);
                let ordinal: i32 = row.get(5);
                entries.push(SchemaSnapshotEntry::Column {
                    parent_schema: row.get(0),
                    parent_table: row.get(1),
                    name: row.get(2),
                    data_type: row.get(3),
                    nullable: nullable_str.map(|s| s == "YES").unwrap_or(true),
                    ordinal: ordinal as usize,
                    default: row.get(6),
                });
            }
        }

        // Indexes
        let idx_query = "SELECT schemaname, tablename, indexname, indexdef FROM pg_indexes \
             WHERE schemaname NOT IN ('pg_catalog', 'information_schema')";
        if let Ok(rows) = self.client.query(idx_query, &[]).await {
            for row in rows {
                let indexdef: String = row.get(3);
                let unique = indexdef.contains("UNIQUE");
                let index_type = if indexdef.contains(" USING gin") {
                    "gin"
                } else if indexdef.contains(" USING hash") {
                    "hash"
                } else {
                    "btree"
                };
                entries.push(SchemaSnapshotEntry::Index {
                    parent_schema: row.get(0),
                    parent_table: row.get(1),
                    name: row.get(2),
                    columns: Vec::new(),
                    unique,
                    index_type: index_type.to_string(),
                });
            }
        }

        Ok(entries)
    }
}
