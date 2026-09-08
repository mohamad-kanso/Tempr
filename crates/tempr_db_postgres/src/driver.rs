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

/// The fingerprint query repeats a scope clause in every segment of a
/// `UNION ALL`, and each segment writes its own placeholders starting back at
/// `$1` (since every clause-builder — `scope_clause`, `function_scope_clause`
/// — numbers from `$1`). This renumbers every segment after the first so the
/// combined query's placeholders are consecutive: segment `i`'s bind values
/// continue where every earlier segment's left off.
///
/// `binds_per_segment[i]` is how many `$N` placeholders segment `i` (0-based,
/// including the first) contributes. Its length must match the number of
/// `UNION ALL` segments in `sql`; on a mismatch `sql` is returned unchanged
/// rather than panicking, since a malformed rewrite is a programmer error in
/// the calling query, not something to fail loudly on for a caller.
///
/// Placeholders are rewritten by scanning digit runs, not by substring
/// replacement, so a `$1` never matches inside a `$10` — substring
/// replacement would corrupt it into `$100` (or similar) the moment any scope
/// ever bound ten or more values.
fn renumber_union_segments(sql: &str, binds_per_segment: &[usize]) -> String {
    let segments: Vec<&str> = sql.split(" UNION ALL ").collect();
    if segments.len() != binds_per_segment.len() {
        return sql.to_string();
    }

    let mut offset = 0usize;
    let mut renumbered = Vec::with_capacity(segments.len());
    for (segment, count) in segments.iter().zip(binds_per_segment) {
        renumbered.push(shift_placeholders(segment, offset));
        offset += count;
    }
    renumbered.join(" UNION ALL ")
}

/// Rewrites every `$<digits>` placeholder in `segment` to `$<digits + offset>`
/// by scanning whole digit runs, so `$1` is never confused with the `$1` that
/// prefixes `$10`.
fn shift_placeholders(segment: &str, offset: usize) -> String {
    if offset == 0 {
        return segment.to_string();
    }
    let mut out = String::with_capacity(segment.len());
    let mut chars = segment.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '$' && chars.peek().is_some_and(char::is_ascii_digit) {
            let mut digits = String::new();
            while let Some(&d) = chars.peek() {
                if d.is_ascii_digit() {
                    digits.push(d);
                    chars.next();
                } else {
                    break;
                }
            }
            out.push('$');
            match digits.parse::<usize>() {
                Ok(n) => out.push_str(&(n + offset).to_string()),
                Err(_) => out.push_str(&digits),
            }
        } else {
            out.push(c);
        }
    }
    out
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

        // Tables, partitioned tables, and foreign tables. Views/materialized
        // views are queried separately below; together the two relkind lists
        // cover the same set as the column query's ('r', 'p', 'v', 'm', 'f').
        let sql = format!(
            "SELECT c.oid::int8, n.nspname, c.relname, c.reltuples::int8 \
             FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace \
             WHERE c.relkind IN ('r', 'p', 'f') AND {where_clause}"
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
        // Four segments, one per `ObjectKind` discriminant (0..3, matching
        // declaration order). Each clause-builder numbers its own
        // placeholders from `$1`; `renumber_union_segments` below makes the
        // combined query's placeholders consecutive. The relation and column
        // segments share `scope_clause(&scope, "n", "c")` and so share a bind
        // count, but the index segment scopes through the parent table (like
        // `snapshot_schema`'s index query) and the function segment uses
        // `function_scope_clause` — both can bind a different number of
        // values than the relation/column segments (e.g. `SchemaScope::Table`
        // binds 2 for a relation/column but only 1 for a function, which has
        // no table to match against).
        let (relation_where, relation_binds) = scope_clause(&scope, "n", "c");
        let (index_where, index_binds) = scope_clause(&scope, "n", "t");
        let (function_where, function_binds) = function_scope_clause(&scope, "n");

        // xmin is the transaction that last wrote the catalog row, so any DDL
        // moves it. A frozen row reports 2, which differs from the cached value
        // and forces a re-introspect — a false positive, never a missed change.
        let sql = format!(
            "SELECT c.oid::int8, 0::int2, c.xmin::text::int8 \
             FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace \
             WHERE c.relkind IN ('r', 'p', 'v', 'm', 'f') AND {relation_where} \
             UNION ALL \
             SELECT (a.attrelid::int8 << 16) | a.attnum::int8, 1::int2, a.xmin::text::int8 \
             FROM pg_attribute a \
             JOIN pg_class c ON c.oid = a.attrelid \
             JOIN pg_namespace n ON n.oid = c.relnamespace \
             WHERE a.attnum > 0 AND NOT a.attisdropped \
               AND c.relkind IN ('r', 'p', 'v', 'm', 'f') AND {relation_where} \
             UNION ALL \
             SELECT i.oid::int8, 2::int2, i.xmin::text::int8 \
             FROM pg_class i \
             JOIN pg_index ix ON ix.indexrelid = i.oid \
             JOIN pg_class t ON t.oid = ix.indrelid \
             JOIN pg_namespace n ON n.oid = t.relnamespace \
             WHERE i.relkind = 'i' AND {index_where} \
             UNION ALL \
             SELECT p.oid::int8, 3::int2, p.xmin::text::int8 \
             FROM pg_proc p \
             JOIN pg_namespace n ON n.oid = p.pronamespace \
             WHERE p.prokind = 'f' AND {function_where}"
        );

        let binds_per_segment = [
            relation_binds.len(),
            relation_binds.len(),
            index_binds.len(),
            function_binds.len(),
        ];
        let sql = renumber_union_segments(&sql, &binds_per_segment);

        let mut all_binds: Vec<String> =
            Vec::with_capacity(2 * relation_binds.len() + index_binds.len() + function_binds.len());
        all_binds.extend(relation_binds.iter().cloned());
        all_binds.extend(relation_binds.iter().cloned());
        all_binds.extend(index_binds.iter().cloned());
        all_binds.extend(function_binds.iter().cloned());
        let params = as_params(&all_binds);

        let rows = self
            .client
            .query(&sql, &params)
            .await
            .map_err(|e| DriverError::Query(e.to_string()))?;

        rows.into_iter()
            .map(|row| {
                let native: i64 = row.get(0);
                let kind: i16 = row.get(1);
                let version: i64 = row.get(2);
                let kind = match kind {
                    0 => ObjectKind::Relation,
                    1 => ObjectKind::Column,
                    2 => ObjectKind::Index,
                    3 => ObjectKind::Function,
                    other => {
                        return Err(DriverError::Internal(format!(
                            "schema_fingerprints: unrecognized object kind discriminant {other}"
                        )));
                    }
                };
                Ok(SchemaFingerprint {
                    native_id: native as u64,
                    kind,
                    version: version as u64,
                })
            })
            .collect()
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
    fn two_segments_with_two_binds_each_are_renumbered() {
        let sql = "SELECT 1 WHERE a = $1 AND b = $2 UNION ALL SELECT 2 WHERE a = $1 AND b = $2";
        let out = renumber_union_segments(sql, &[2, 2]);
        assert_eq!(
            out,
            "SELECT 1 WHERE a = $1 AND b = $2 UNION ALL SELECT 2 WHERE a = $3 AND b = $4"
        );
    }

    #[test]
    fn four_segments_with_two_binds_each_are_renumbered() {
        let sql = "SELECT 1 WHERE a = $1 AND b = $2 \
                    UNION ALL SELECT 2 WHERE a = $1 AND b = $2 \
                    UNION ALL SELECT 3 WHERE a = $1 AND b = $2 \
                    UNION ALL SELECT 4 WHERE a = $1 AND b = $2";
        let out = renumber_union_segments(sql, &[2, 2, 2, 2]);
        assert_eq!(
            out,
            "SELECT 1 WHERE a = $1 AND b = $2 \
             UNION ALL SELECT 2 WHERE a = $3 AND b = $4 \
             UNION ALL SELECT 3 WHERE a = $5 AND b = $6 \
             UNION ALL SELECT 4 WHERE a = $7 AND b = $8"
        );
    }

    #[test]
    fn segments_with_uneven_bind_counts_are_renumbered_by_running_offset() {
        // Mirrors `schema_fingerprints`: the relation and column segments
        // each bind 2 values, the index segment binds 2 (scoped through the
        // parent table), and the function segment binds only 1 — so offsets
        // must accumulate per-segment, not as a uniform `i * count`.
        let sql = "SELECT 1 WHERE a = $1 AND b = $2 \
                    UNION ALL SELECT 2 WHERE a = $1 AND b = $2 \
                    UNION ALL SELECT 3 WHERE a = $1 AND b = $2 \
                    UNION ALL SELECT 4 WHERE a = $1";
        let out = renumber_union_segments(sql, &[2, 2, 2, 1]);
        assert_eq!(
            out,
            "SELECT 1 WHERE a = $1 AND b = $2 \
             UNION ALL SELECT 2 WHERE a = $3 AND b = $4 \
             UNION ALL SELECT 3 WHERE a = $5 AND b = $6 \
             UNION ALL SELECT 4 WHERE a = $7"
        );
    }

    #[test]
    fn renumbering_is_a_no_op_without_binds() {
        let sql = "SELECT 1 UNION ALL SELECT 2";
        assert_eq!(renumber_union_segments(sql, &[0, 0]), sql);
    }

    #[test]
    fn ten_placeholders_in_one_segment_are_not_corrupted_by_digit_prefix_matching() {
        // The first segment binds 1 value; the second binds 10 ($1..$10). A
        // substring-replace renumbering (the old implementation) would turn
        // "$1" into "$2" as a blind text swap and hit the "$1" that prefixes
        // "$10", corrupting it. Scanning digit runs must shift $1 -> $2 and
        // $10 -> $11 without cross-contamination.
        let sql = "SELECT 1 WHERE a = $1 \
                    UNION ALL SELECT 2 WHERE a = $1 AND b = $2 AND c = $3 AND d = $4 \
                    AND e = $5 AND f = $6 AND g = $7 AND h = $8 AND i = $9 AND j = $10";
        let out = renumber_union_segments(sql, &[1, 10]);
        assert_eq!(
            out,
            "SELECT 1 WHERE a = $1 \
             UNION ALL SELECT 2 WHERE a = $2 AND b = $3 AND c = $4 AND d = $5 \
             AND e = $6 AND f = $7 AND g = $8 AND h = $9 AND i = $10 AND j = $11"
        );
    }

    #[test]
    fn mismatched_segment_count_returns_sql_unchanged() {
        let sql = "SELECT 1 WHERE a = $1 UNION ALL SELECT 2 WHERE a = $1";
        assert_eq!(renumber_union_segments(sql, &[1, 1, 1]), sql);
    }
}
