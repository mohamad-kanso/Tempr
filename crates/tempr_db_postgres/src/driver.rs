use tempr_db::stream::QueryStream;
use tempr_db::{
    CancelHandle, DatabaseDriver, DriverConnection, DriverError, EngineId, SchemaScope,
    SchemaSnapshotEntry,
};
use tempr_domain::{ColumnSpec, Connection, Value};
use tokio_postgres::NoTls;
use tokio_postgres::config::SslMode;

use crate::params::{as_sql_refs, to_sql_params};
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
        let mut config = tokio_postgres::Config::new();
        config
            .host(&connection.host)
            .port(connection.port)
            .dbname(&connection.database)
            .user(&connection.username)
            .password(&connection.password)
            // No TLS connector is wired up yet (see docs/09-database-engine.md
            // follow-up) — `Prefer` communicates intent without breaking
            // today's plaintext-only connections the way `Require` would.
            .ssl_mode(SslMode::Prefer);

        let (client, connection_handle) = config.connect(NoTls).await.map_err(|e| {
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
                tracing::error!("Postgres connection task failed: {e:?}");
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

struct PostgresCancelHandle(tokio_postgres::CancelToken);

#[async_trait::async_trait]
impl CancelHandle for PostgresCancelHandle {
    async fn cancel(&self) -> Result<(), DriverError> {
        self.0
            .cancel_query(NoTls)
            .await
            .map_err(|e| DriverError::Internal(format!("cancel failed: {e}")))
    }
}

#[async_trait::async_trait]
impl DriverConnection for PostgresConnection {
    async fn execute(&mut self, sql: &str, params: &[Value]) -> Result<QueryStream, DriverError> {
        let owned_params = to_sql_params(params);
        let sql_params = as_sql_refs(&owned_params);

        let stmt = self
            .client
            .prepare(sql)
            .await
            .map_err(|e| DriverError::Query(e.to_string()))?;

        if stmt.columns().is_empty() {
            // No result columns: DDL/DML with no RETURNING clause.
            let rows_affected = self
                .client
                .execute(&stmt, &sql_params)
                .await
                .map_err(|e| DriverError::Query(e.to_string()))?;
            return Ok(QueryStream::new(
                Box::new(PostgresStream::for_dml(rows_affected)),
                self.batch_size,
            ));
        }

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

        let rows = self
            .client
            .query(&stmt, &sql_params)
            .await
            .map_err(|e| DriverError::Query(e.to_string()))?;

        Ok(QueryStream::new(
            Box::new(PostgresStream::from_rows(columns, rows, self.batch_size)),
            self.batch_size,
        ))
    }

    async fn cancel(&mut self) -> Result<(), DriverError> {
        self.cancel_handle().cancel().await
    }

    fn cancel_handle(&self) -> Box<dyn CancelHandle> {
        Box::new(PostgresCancelHandle(self.client.cancel_token()))
    }

    async fn snapshot_schema(
        &mut self,
        scope: SchemaScope,
    ) -> Result<Vec<SchemaSnapshotEntry>, DriverError> {
        let mut entries = Vec::new();

        // Tables — parameterized to prevent SQL injection and to honor scope.
        match &scope {
            SchemaScope::All => {
                let rows = self
                    .client
                    .query(
                        "SELECT schemaname, tablename FROM pg_tables \
                         WHERE schemaname NOT IN ('pg_catalog', 'information_schema')",
                        &[],
                    )
                    .await
                    .map_err(|e| DriverError::Query(e.to_string()))?;
                for row in rows {
                    entries.push(SchemaSnapshotEntry::Table {
                        schema: row.get(0),
                        name: row.get(1),
                        estimated_rows: None,
                    });
                }
            }
            SchemaScope::Schema(s) => {
                let rows = self
                    .client
                    .query(
                        "SELECT schemaname, tablename FROM pg_tables WHERE schemaname = $1",
                        &[s],
                    )
                    .await
                    .map_err(|e| DriverError::Query(e.to_string()))?;
                for row in rows {
                    entries.push(SchemaSnapshotEntry::Table {
                        schema: row.get(0),
                        name: row.get(1),
                        estimated_rows: None,
                    });
                }
            }
            SchemaScope::Table { schema, table } => {
                let rows = self
                    .client
                    .query(
                        "SELECT schemaname, tablename FROM pg_tables \
                         WHERE schemaname = $1 AND tablename = $2",
                        &[schema, table],
                    )
                    .await
                    .map_err(|e| DriverError::Query(e.to_string()))?;
                for row in rows {
                    entries.push(SchemaSnapshotEntry::Table {
                        schema: row.get(0),
                        name: row.get(1),
                        estimated_rows: None,
                    });
                }
            }
        }

        // Columns — same scope filter as the table query above.
        let col_rows = match &scope {
            SchemaScope::All => {
                self.client
                    .query(
                        "SELECT table_schema, table_name, column_name, data_type, \
                     is_nullable, ordinal_position, column_default \
                     FROM information_schema.columns \
                     WHERE table_schema NOT IN ('pg_catalog', 'information_schema') \
                     ORDER BY table_schema, table_name, ordinal_position",
                        &[],
                    )
                    .await
            }
            SchemaScope::Schema(s) => {
                self.client
                    .query(
                        "SELECT table_schema, table_name, column_name, data_type, \
                     is_nullable, ordinal_position, column_default \
                     FROM information_schema.columns \
                     WHERE table_schema = $1 \
                     ORDER BY table_schema, table_name, ordinal_position",
                        &[s],
                    )
                    .await
            }
            SchemaScope::Table { schema, table } => {
                self.client
                    .query(
                        "SELECT table_schema, table_name, column_name, data_type, \
                     is_nullable, ordinal_position, column_default \
                     FROM information_schema.columns \
                     WHERE table_schema = $1 AND table_name = $2 \
                     ORDER BY table_schema, table_name, ordinal_position",
                        &[schema, table],
                    )
                    .await
            }
        }
        .map_err(|e| DriverError::Query(e.to_string()))?;

        for row in col_rows {
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

        // Indexes — join pg_index/pg_attribute directly for exact, ordered
        // column names instead of parsing `indexdef` text (which breaks on
        // expression indexes and multi-word index types).
        const IDX_SELECT: &str = "SELECT n.nspname, t.relname, i.relname, ix.indisunique, \
             am.amname, array_agg(a.attname ORDER BY x.ordinality) \
             FROM pg_index ix \
             JOIN pg_class i ON i.oid = ix.indexrelid \
             JOIN pg_class t ON t.oid = ix.indrelid \
             JOIN pg_namespace n ON n.oid = t.relnamespace \
             JOIN pg_am am ON am.oid = i.relam \
             JOIN LATERAL unnest(ix.indkey) WITH ORDINALITY AS x(attnum, ordinality) ON true \
             JOIN pg_attribute a ON a.attrelid = t.oid AND a.attnum = x.attnum";
        const IDX_GROUP_BY: &str =
            "GROUP BY n.nspname, t.relname, i.relname, ix.indisunique, am.amname";

        let idx_rows = match &scope {
            SchemaScope::All => {
                let sql = format!(
                    "{IDX_SELECT} WHERE n.nspname NOT IN ('pg_catalog', 'information_schema') {IDX_GROUP_BY}"
                );
                self.client.query(&sql, &[]).await
            }
            SchemaScope::Schema(s) => {
                let sql = format!("{IDX_SELECT} WHERE n.nspname = $1 {IDX_GROUP_BY}");
                self.client.query(&sql, &[s]).await
            }
            SchemaScope::Table { schema, table } => {
                let sql =
                    format!("{IDX_SELECT} WHERE n.nspname = $1 AND t.relname = $2 {IDX_GROUP_BY}");
                self.client.query(&sql, &[schema, table]).await
            }
        }
        .map_err(|e| DriverError::Query(e.to_string()))?;

        for row in idx_rows {
            let unique: bool = row.get(3);
            let index_type: String = row.get(4);
            let columns: Vec<String> = row.get(5);
            entries.push(SchemaSnapshotEntry::Index {
                parent_schema: row.get(0),
                parent_table: row.get(1),
                name: row.get(2),
                columns,
                unique,
                index_type,
            });
        }

        Ok(entries)
    }
}
