use tempr_db::stream::QueryStream;
use tempr_db::{
    CancelHandle, DatabaseDriver, DriverConnection, DriverError, EngineId, ObjectKind,
    SchemaFingerprint, SchemaScope, SchemaSnapshotEntry,
};
use tempr_domain::{ColumnSpec, Connection, TlsMode, Value};
use tokio_postgres::NoTls;
use tokio_postgres::config::SslMode;
use tokio_postgres::error::SqlState;
use tokio_postgres_rustls::MakeRustlsConnect;

use crate::params::{as_sql_refs, to_sql_params};
use crate::stream::PostgresStream;
use crate::tls::{connector, ssl_mode};

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

/// TCP connect timeout for new connections and cancel sockets.
const CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

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
            .ssl_mode(ssl_mode(connection.tls))
            // Bounds the TCP connect only (not the TLS handshake) so an
            // unreachable host fails fast instead of waiting for the OS SYN
            // timeout. Callers bound the whole operation (pool create
            // timeout, cancel timeout).
            .connect_timeout(CONNECT_TIMEOUT);

        let tls = connector(connection.tls).await?;
        let client = match config.connect(tls.clone()).await {
            Ok((client, conn)) => {
                spawn_connection(conn);
                client
            }
            // libpq `prefer`: a server that offers TLS but whose handshake we
            // cannot complete still gets a plaintext connection.
            Err(e) if connection.tls == TlsMode::Prefer && is_tls_error(&e) => {
                tracing::warn!(
                    error = %describe(&e),
                    "TLS handshake failed with sslmode=prefer; retrying in plaintext"
                );
                config.ssl_mode(SslMode::Disable);
                let (client, conn) = config.connect(NoTls).await.map_err(classify)?;
                spawn_connection(conn);
                client
            }
            Err(e) => return Err(classify(e)),
        };

        Ok(Box::new(PostgresConnection {
            client,
            tls,
            batch_size: DEFAULT_BATCH_SIZE,
        }))
    }
}

/// Drive the connection's I/O task to completion in the background.
fn spawn_connection<F>(conn: F)
where
    F: std::future::Future<Output = Result<(), tokio_postgres::Error>> + Send + 'static,
{
    tokio::spawn(async move {
        if let Err(e) = conn.await {
            tracing::error!("Postgres connection task failed: {e:?}");
        }
    });
}

/// `Display` of a tokio-postgres error is only the kind ("db error", "error
/// performing TLS handshake"); the useful part is in the source chain.
fn describe(e: &tokio_postgres::Error) -> String {
    let mut parts = vec![e.to_string()];
    let mut cur: Option<&(dyn std::error::Error + 'static)> = std::error::Error::source(e);
    while let Some(err) = cur {
        parts.push(err.to_string());
        cur = err.source();
    }
    parts.join(": ")
}

fn is_tls_error(e: &tokio_postgres::Error) -> bool {
    e.to_string().starts_with("error performing TLS handshake")
}

/// Map a connect-time error onto the driver error taxonomy.
fn classify(e: tokio_postgres::Error) -> DriverError {
    let text = describe(&e);
    match e.as_db_error().map(|d| d.code()) {
        Some(code)
            if *code == SqlState::INVALID_PASSWORD
                || *code == SqlState::INVALID_AUTHORIZATION_SPECIFICATION =>
        {
            DriverError::AuthFailed(text)
        }
        _ if is_tls_error(&e) => DriverError::ConnectionRefused(format!("TLS: {text}")),
        _ => DriverError::ConnectionRefused(text),
    }
}

struct PostgresConnection {
    client: tokio_postgres::Client,
    tls: MakeRustlsConnect,
    batch_size: usize,
}

struct PostgresCancelHandle {
    token: tokio_postgres::CancelToken,
    tls: MakeRustlsConnect,
}

#[async_trait::async_trait]
impl CancelHandle for PostgresCancelHandle {
    async fn cancel(&self) -> Result<(), DriverError> {
        self.token
            .cancel_query(self.tls.clone())
            .await
            .map_err(|e| DriverError::Internal(format!("cancel failed: {}", describe(&e))))
    }
}

/// SQL fragment restricting a catalog query to `scope`, plus the values to
/// bind. `ns` and `rel` are the aliases of the `pg_namespace` and `pg_class`
/// rows in the calling query. Bind values start at `$1`.
fn scope_clause(scope: &SchemaScope, ns: &str, rel: &str) -> (String, Vec<String>) {
    match scope {
        SchemaScope::SearchPath => (
            format!("({ns}.nspname = ANY (current_schemas(false)) OR {ns}.nspname = 'public')"),
            Vec::new(),
        ),
        SchemaScope::All => (
            format!("{ns}.nspname NOT IN ('pg_catalog', 'information_schema')"),
            Vec::new(),
        ),
        SchemaScope::Schema(s) => (format!("{ns}.nspname = $1"), vec![s.clone()]),
        SchemaScope::Table { schema, table } => (
            format!("{ns}.nspname = $1 AND {rel}.relname = $2"),
            vec![schema.clone(), table.clone()],
        ),
    }
}

/// Scope clause for `pg_proc` queries. `ns` is the alias of the joined
/// `pg_namespace` row. `SchemaScope::Table` degrades to its schema, since a
/// function is not owned by a table.
fn function_scope_clause(scope: &SchemaScope, ns: &str) -> (String, Vec<String>) {
    match scope {
        SchemaScope::SearchPath => (
            format!("({ns}.nspname = ANY (current_schemas(false)) OR {ns}.nspname = 'public')"),
            Vec::new(),
        ),
        SchemaScope::All => (
            format!("{ns}.nspname NOT IN ('pg_catalog', 'information_schema')"),
            Vec::new(),
        ),
        SchemaScope::Schema(s) => (format!("{ns}.nspname = $1"), vec![s.clone()]),
        SchemaScope::Table { schema, .. } => (format!("{ns}.nspname = $1"), vec![schema.clone()]),
    }
}

/// The fingerprint query repeats its scope clause in both halves of a UNION,
/// so the second half's placeholders must continue where the first left off:
/// `$1, $2` become `$3, $4`. `count` is how many binds one clause uses.
fn renumber_second_clause(sql: &str, count: usize) -> String {
    if count == 0 {
        return sql.to_string();
    }
    let (head, tail) = match sql.split_once(" UNION ALL ") {
        Some(parts) => parts,
        None => return sql.to_string(),
    };
    let mut renumbered = tail.to_string();
    // Rewrite from the highest placeholder down, so $1 -> $3 never collides
    // with an existing $2 that still has to move.
    for i in (1..=count).rev() {
        renumbered = renumbered.replace(&format!("${i}"), &format!("${}", i + count));
    }
    format!("{head} UNION ALL {renumbered}")
}

/// Borrow bind values as `tokio_postgres` parameters.
fn as_params(values: &[String]) -> Vec<&(dyn tokio_postgres::types::ToSql + Sync)> {
    values
        .iter()
        .map(|v| v as &(dyn tokio_postgres::types::ToSql + Sync))
        .collect()
}

#[async_trait::async_trait]
impl DriverConnection for PostgresConnection {
    fn is_closed(&self) -> bool {
        self.client.is_closed()
    }

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
        Box::new(PostgresCancelHandle {
            token: self.client.cancel_token(),
            tls: self.tls.clone(),
        })
    }

    async fn snapshot_schema(
        &mut self,
        scope: SchemaScope,
    ) -> Result<Vec<SchemaSnapshotEntry>, DriverError> {
        let mut entries = Vec::new();
        let (where_clause, binds) = scope_clause(&scope, "n", "c");
        let params = as_params(&binds);

        // Tables and partitioned tables.
        let sql = format!(
            "SELECT c.oid::int8, n.nspname, c.relname, c.reltuples::int8 \
             FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace \
             WHERE c.relkind IN ('r', 'p') AND {where_clause}"
        );
        for row in self
            .client
            .query(&sql, &params)
            .await
            .map_err(|e| DriverError::Query(e.to_string()))?
        {
            let oid: i64 = row.get(0);
            let reltuples: i64 = row.get(3);
            entries.push(SchemaSnapshotEntry::Table {
                native_id: oid as u64,
                schema: row.get(1),
                name: row.get(2),
                // reltuples is -1 until the relation has been analyzed.
                estimated_rows: (reltuples >= 0).then_some(reltuples as u64),
            });
        }

        // Views and materialized views.
        let sql = format!(
            "SELECT c.oid::int8, n.nspname, c.relname, pg_get_viewdef(c.oid, true) \
             FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace \
             WHERE c.relkind IN ('v', 'm') AND {where_clause}"
        );
        for row in self
            .client
            .query(&sql, &params)
            .await
            .map_err(|e| DriverError::Query(e.to_string()))?
        {
            let oid: i64 = row.get(0);
            entries.push(SchemaSnapshotEntry::View {
                native_id: oid as u64,
                schema: row.get(1),
                name: row.get(2),
                definition: row.get(3),
            });
        }

        // Columns of every relation kind that has them.
        let sql = format!(
            "SELECT (a.attrelid::int8 << 16) | a.attnum::int8, n.nspname, c.relname, a.attname, \
                    format_type(a.atttypid, a.atttypmod), a.attnotnull, a.attnum, \
                    pg_get_expr(d.adbin, d.adrelid) \
             FROM pg_attribute a \
             JOIN pg_class c ON c.oid = a.attrelid \
             JOIN pg_namespace n ON n.oid = c.relnamespace \
             LEFT JOIN pg_attrdef d ON d.adrelid = a.attrelid AND d.adnum = a.attnum \
             WHERE a.attnum > 0 AND NOT a.attisdropped \
               AND c.relkind IN ('r', 'p', 'v', 'm', 'f') AND {where_clause} \
             ORDER BY n.nspname, c.relname, a.attnum"
        );
        for row in self
            .client
            .query(&sql, &params)
            .await
            .map_err(|e| DriverError::Query(e.to_string()))?
        {
            let native: i64 = row.get(0);
            let not_null: bool = row.get(5);
            let attnum: i16 = row.get(6);
            entries.push(SchemaSnapshotEntry::Column {
                native_id: native as u64,
                parent_schema: row.get(1),
                parent_table: row.get(2),
                name: row.get(3),
                data_type: row.get(4),
                nullable: !not_null,
                ordinal: attnum as usize,
                default: row.get(7),
            });
        }

        // Indexes — pg_index join for exact, ordered column names.
        let sql = format!(
            "SELECT i.oid::int8, n.nspname, t.relname, i.relname, ix.indisunique, am.amname, \
                    array_agg(a.attname ORDER BY x.ordinality) \
             FROM pg_index ix \
             JOIN pg_class i ON i.oid = ix.indexrelid \
             JOIN pg_class t ON t.oid = ix.indrelid \
             JOIN pg_namespace n ON n.oid = t.relnamespace \
             JOIN pg_am am ON am.oid = i.relam \
             JOIN LATERAL unnest(ix.indkey) WITH ORDINALITY AS x(attnum, ordinality) ON true \
             JOIN pg_attribute a ON a.attrelid = t.oid AND a.attnum = x.attnum \
             WHERE {} \
             GROUP BY i.oid, n.nspname, t.relname, i.relname, ix.indisunique, am.amname",
            scope_clause(&scope, "n", "t").0
        );
        for row in self
            .client
            .query(&sql, &params)
            .await
            .map_err(|e| DriverError::Query(e.to_string()))?
        {
            let oid: i64 = row.get(0);
            let unique: bool = row.get(4);
            let index_type: String = row.get(5);
            let columns: Vec<String> = row.get(6);
            entries.push(SchemaSnapshotEntry::Index {
                native_id: oid as u64,
                parent_schema: row.get(1),
                parent_table: row.get(2),
                name: row.get(3),
                columns,
                unique,
                index_type,
            });
        }

        // Plain functions only: 'p' is a procedure, 'a' an aggregate, 'w' a window
        // function — none of which complete like a scalar call.
        let (fn_where, fn_binds) = function_scope_clause(&scope, "n");
        let fn_params = as_params(&fn_binds);
        let sql = format!(
            "SELECT p.oid::int8, n.nspname, p.proname, \
                    args.names, args.types, \
                    format_type(p.prorettype, NULL), l.lanname \
             FROM pg_proc p \
             JOIN pg_namespace n ON n.oid = p.pronamespace \
             JOIN pg_language l ON l.oid = p.prolang \
             LEFT JOIN LATERAL ( \
                 SELECT COALESCE(array_agg(COALESCE(p.proargnames[u.ord], '') ORDER BY u.ord), ARRAY[]::text[]) AS names, \
                        COALESCE(array_agg(format_type(u.t, NULL) ORDER BY u.ord), ARRAY[]::text[]) AS types \
                 FROM unnest(COALESCE(p.proallargtypes, p.proargtypes::oid[])) WITH ORDINALITY AS u(t, ord) \
                 WHERE COALESCE(p.proargmodes[u.ord], 'i') IN ('i', 'b', 'v') \
             ) args ON true \
             WHERE p.prokind = 'f' AND {fn_where} \
             ORDER BY n.nspname, p.proname"
        );
        for row in self
            .client
            .query(&sql, &fn_params)
            .await
            .map_err(|e| DriverError::Query(e.to_string()))?
        {
            let oid: i64 = row.get(0);
            let arg_names: Vec<String> = row.get(3);
            let arg_types: Vec<String> = row.get(4);
            let parameters = arg_types
                .into_iter()
                .enumerate()
                .map(|(i, ty)| {
                    let name = arg_names
                        .get(i)
                        .filter(|n| !n.is_empty())
                        .cloned()
                        .unwrap_or_else(|| format!("${}", i + 1));
                    (name, ty)
                })
                .collect();
            entries.push(SchemaSnapshotEntry::Function {
                native_id: oid as u64,
                schema: row.get(1),
                name: row.get(2),
                parameters,
                return_type: row.get(5),
                language: row.get(6),
            });
        }

        Ok(entries)
    }

    async fn schema_fingerprints(
        &mut self,
        scope: SchemaScope,
    ) -> Result<Vec<SchemaFingerprint>, DriverError> {
        let (where_clause, binds) = scope_clause(&scope, "n", "c");
        let params = as_params(&binds);

        // xmin is the transaction that last wrote the catalog row, so any DDL
        // moves it. A frozen row reports 2, which differs from the cached value
        // and forces a re-introspect — a false positive, never a missed change.
        let sql = format!(
            "SELECT c.oid::int8, 0::int2, c.xmin::text::int8 \
             FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace \
             WHERE c.relkind IN ('r', 'p', 'v', 'm', 'f') AND {where_clause} \
             UNION ALL \
             SELECT (a.attrelid::int8 << 16) | a.attnum::int8, 1::int2, a.xmin::text::int8 \
             FROM pg_attribute a \
             JOIN pg_class c ON c.oid = a.attrelid \
             JOIN pg_namespace n ON n.oid = c.relnamespace \
             WHERE a.attnum > 0 AND NOT a.attisdropped \
               AND c.relkind IN ('r', 'p', 'v', 'm', 'f') AND {where_clause}"
        );

        // The clause appears twice, so the binds do too.
        let mut doubled: Vec<&(dyn tokio_postgres::types::ToSql + Sync)> = Vec::new();
        doubled.extend_from_slice(&params);
        doubled.extend_from_slice(&params);
        let sql = renumber_second_clause(&sql, binds.len());

        let rows = self
            .client
            .query(&sql, &doubled)
            .await
            .map_err(|e| DriverError::Query(e.to_string()))?;

        Ok(rows
            .into_iter()
            .map(|row| {
                let native: i64 = row.get(0);
                let kind: i16 = row.get(1);
                let version: i64 = row.get(2);
                SchemaFingerprint {
                    native_id: native as u64,
                    kind: if kind == 0 {
                        ObjectKind::Relation
                    } else {
                        ObjectKind::Column
                    },
                    version: version as u64,
                }
            })
            .collect())
    }

    async fn keywords(&mut self) -> Result<Vec<String>, DriverError> {
        let rows = self
            .client
            .query("SELECT word FROM pg_get_keywords()", &[])
            .await
            .map_err(|e| DriverError::Query(e.to_string()))?;
        Ok(rows.into_iter().map(|row| row.get(0)).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn second_union_clause_placeholders_are_renumbered() {
        let sql = "SELECT 1 WHERE a = $1 AND b = $2 UNION ALL SELECT 2 WHERE a = $1 AND b = $2";
        let out = renumber_second_clause(sql, 2);
        assert_eq!(
            out,
            "SELECT 1 WHERE a = $1 AND b = $2 UNION ALL SELECT 2 WHERE a = $3 AND b = $4"
        );
    }

    #[test]
    fn renumbering_is_a_no_op_without_binds() {
        let sql = "SELECT 1 UNION ALL SELECT 2";
        assert_eq!(renumber_second_clause(sql, 0), sql);
    }
}
