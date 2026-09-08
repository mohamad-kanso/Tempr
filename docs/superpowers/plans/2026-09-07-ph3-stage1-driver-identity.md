# Phase 3 Stage 1 — Driver Identity Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give the database driver layer everything the catalog cache needs: stable native ids on every schema object, function metadata, a cheap change-detection sweep, the server's keyword list, and a `search_path`-scoped introspection mode.

**Architecture:** PostgreSQL introspection moves off `pg_tables` / `information_schema` onto `pg_catalog` (`pg_class`, `pg_namespace`, `pg_attribute`, `pg_proc`), because only the catalog exposes OIDs. `SchemaSnapshotEntry` grows a `native_id: u64` on every variant plus a `Function` variant; `DriverConnection` grows `schema_fingerprints` and `keywords`, both with default implementations so non-PostgreSQL drivers compile untouched. Nothing in this stage consumes the new data — `SchemaService` keeps behaving exactly as it does today; stage 2 turns it into identity and cache.

**Tech Stack:** Rust 1.97.1, `tokio-postgres`, `async-trait`, existing `tempr_db` / `tempr_db_postgres` / `tempr_services` crates.

**Spec:** [docs/superpowers/specs/2026-09-07-phase3-sql-intelligence-design.md](../specs/2026-09-07-phase3-sql-intelligence-design.md) — §3 (object identity), §4.3 (refresh), §4.4 (catalog scope), §4.5 (keywords), §9 stage 1.

## Global Constraints

- **Branch, PR, review — never commit to main.** Branch name for this stage: `feat/ph3-driver-identity`. See CLAUDE.md hard rules.
- **Every task ends green**: `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`.
- **Integration tests need a live PostgreSQL.** Start it and export the URL before running anything marked `--ignored`:
  ```bash
  docker start tempr-pg && export DATABASE_URL='postgres://tempr:tempr@localhost:55432/tempr?sslmode=disable'
  ```
  Run them with `cargo test --workspace -- --ignored`. Tests that need a live server carry `#[ignore = "requires DATABASE_URL env var pointing to a live PostgreSQL instance"]`, matching the existing file.
- **No new dependencies.** Everything here uses crates already in the graph. A new dependency would need a DECISIONS.md entry (D18 rule).
- **SQL is parameterized.** Scope values are bound as `$1`/`$2`, never formatted into the statement. The only `format!` on SQL is for fixed, code-authored fragments — the pattern already used in `snapshot_schema`.
- **No `unwrap` / `expect` in library code** (`#![deny(unsafe_code)]` and the clippy config already enforce the house style); tests may use them, and the integration test file already opts in at the top.
- **Naming is canonical**: `native_id`, `SchemaFingerprint`, `ObjectKind`, `SchemaScope::SearchPath`, `DriverError::Unsupported`. These names appear in the spec and in later stages — do not rename them.

---

## File Structure

| File | Responsibility in this stage |
|---|---|
| `crates/tempr_db/src/driver.rs` | `SchemaScope::SearchPath`; `native_id` on every `SchemaSnapshotEntry`; new `Function` variant; `SchemaFingerprint` + `ObjectKind`; `DriverConnection::schema_fingerprints` and `::keywords` with defaults |
| `crates/tempr_db/src/error.rs` | New `DriverError::Unsupported` variant |
| `crates/tempr_db/src/lib.rs` | Re-export the new public types |
| `crates/tempr_db_postgres/src/driver.rs` | Catalog-based introspection with OIDs, function introspection, fingerprint sweep, keyword query, `SearchPath` scope clause |
| `crates/tempr_services/src/schema.rs` | Match arms updated for the new field and variant (behaviour unchanged this stage) |
| `crates/tempr/tests/integration.rs` | Integration tests for every new driver capability |
| `docs/09-database-engine.md` | Driver trait documentation for the additions |
| `docs/PROGRESS.md`, `docs/TODO.md` | Status block, session log, and the deferred items this stage names |

---

## Task 1: Catalog-based introspection with native ids

**Files:**
- Modify: `crates/tempr_db/src/driver.rs` (the `SchemaSnapshotEntry` enum, ~line 75)
- Modify: `crates/tempr_db_postgres/src/driver.rs` (`snapshot_schema`, ~lines 219-390)
- Modify: `crates/tempr_services/src/schema.rs` (match arms in `refresh`, ~lines 40-120)
- Test: `crates/tempr/tests/integration.rs`

**Interfaces:**
- Consumes: nothing (first task)
- Produces: `SchemaSnapshotEntry::{Table,View,Column,Index}` each carrying `native_id: u64`. Column ids are `(attrelid << 16) | attnum`; relation ids are `pg_class.oid`. Later tasks and stage 2 rely on these being stable across refreshes.

- [ ] **Step 1: Write the failing test**

Add to `crates/tempr/tests/integration.rs`:

```rust
#[tokio::test]
#[ignore = "requires DATABASE_URL env var pointing to a live PostgreSQL instance"]
async fn pg_snapshot_entries_carry_stable_native_ids() {
    let (_bus, cs) = setup_pg_cs();
    let id = connect_test_pg(&cs).await;

    cs.with_metadata_connection_fn(id, |mut conn| async move {
        conn.execute("DROP TABLE IF EXISTS native_id_probe", &[]).await?;
        conn.execute(
            "CREATE TABLE native_id_probe (id bigint primary key, label text not null)",
            &[],
        )
        .await
    })
    .await
    .expect("setup table");

    let first = cs
        .with_metadata_connection_fn(id, |mut conn| async move {
            conn.snapshot_schema(SchemaScope::All).await
        })
        .await
        .expect("first snapshot");
    let second = cs
        .with_metadata_connection_fn(id, |mut conn| async move {
            conn.snapshot_schema(SchemaScope::All).await
        })
        .await
        .expect("second snapshot");

    let table_id = |entries: &[SchemaSnapshotEntry]| -> u64 {
        entries
            .iter()
            .find_map(|e| match e {
                SchemaSnapshotEntry::Table { native_id, name, .. } if name == "native_id_probe" => {
                    Some(*native_id)
                }
                _ => None,
            })
            .expect("probe table missing from snapshot")
    };
    let a = table_id(&first);
    let b = table_id(&second);
    assert_ne!(a, 0, "native_id must be a real OID");
    assert_eq!(a, b, "native_id must be stable across refreshes");

    // Columns encode (attrelid << 16 | attnum) — same relation, distinct ids.
    let mut col_ids: Vec<u64> = first
        .iter()
        .filter_map(|e| match e {
            SchemaSnapshotEntry::Column { native_id, parent_table, .. }
                if parent_table == "native_id_probe" =>
            {
                Some(*native_id)
            }
            _ => None,
        })
        .collect();
    col_ids.sort_unstable();
    assert_eq!(col_ids.len(), 2, "expected two columns on the probe table");
    assert_ne!(col_ids[0], col_ids[1]);
    assert_eq!(col_ids[0] >> 16, a, "column ids embed their relation OID");

    cs.with_metadata_connection_fn(id, |mut conn| async move {
        conn.execute("DROP TABLE native_id_probe", &[]).await
    })
    .await
    .expect("cleanup");
}
```

Add `SchemaScope` and `SchemaSnapshotEntry` to the file's imports:

```rust
use tempr_db::{SchemaScope, SchemaSnapshotEntry};
```

- [ ] **Step 2: Run the test to verify it fails**

```bash
cargo test --workspace -- --ignored pg_snapshot_entries_carry_stable_native_ids
```

Expected: compile error — `SchemaSnapshotEntry::Table` has no field `native_id`.

- [ ] **Step 3: Add `native_id` to the entry enum**

In `crates/tempr_db/src/driver.rs`, replace the `SchemaSnapshotEntry` enum with:

```rust
/// A single entry in a schema snapshot — flat list with implicit parent-child.
///
/// `native_id` is the database engine's own identifier for the object and must
/// be stable across refreshes and restarts: PostgreSQL uses `pg_class.oid` for
/// relations, `pg_proc.oid` for functions, and `(attrelid << 16) | attnum` for
/// columns. A driver whose engine has no stable identifier should hash the
/// object's qualified name into this field instead; the catalog then treats a
/// rename as a delete plus an insert, which is correct but coarser.
#[derive(Debug, Clone)]
pub enum SchemaSnapshotEntry {
    Table {
        native_id: u64,
        schema: String,
        name: String,
        estimated_rows: Option<u64>,
    },
    View {
        native_id: u64,
        schema: String,
        name: String,
        definition: String,
    },
    Column {
        native_id: u64,
        parent_schema: String,
        parent_table: String,
        name: String,
        data_type: String,
        nullable: bool,
        ordinal: usize,
        default: Option<String>,
    },
    Index {
        native_id: u64,
        parent_schema: String,
        parent_table: String,
        name: String,
        columns: Vec<String>,
        unique: bool,
        index_type: String,
    },
}
```

Every existing field name stays exactly as it is; `native_id` is the only addition.

- [ ] **Step 4: Rewrite PostgreSQL introspection onto `pg_catalog`**

In `crates/tempr_db_postgres/src/driver.rs`, add this helper above the `impl DriverConnection for PostgresConnection` block:

```rust
/// SQL fragment restricting a catalog query to `scope`, plus the values to
/// bind. `ns` and `rel` are the aliases of the `pg_namespace` and `pg_class`
/// rows in the calling query. Bind values start at `$1`.
fn scope_clause(scope: &SchemaScope, ns: &str, rel: &str) -> (String, Vec<String>) {
    match scope {
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

/// Borrow bind values as `tokio_postgres` parameters.
fn as_params(values: &[String]) -> Vec<&(dyn tokio_postgres::types::ToSql + Sync)> {
    values
        .iter()
        .map(|v| v as &(dyn tokio_postgres::types::ToSql + Sync))
        .collect()
}
```

Replace the body of `snapshot_schema` with:

```rust
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

        Ok(entries)
    }
```

Delete the now-unused `IDX_SELECT` / `IDX_GROUP_BY` constants.

- [ ] **Step 5: Update the `SchemaService` match arms**

In `crates/tempr_services/src/schema.rs`, the `refresh` method destructures entries. The `Table` and `View` arms already end in `..` and need no change. The `Column` arm (~line 72) and the `Index` arm (~line 95) list every field explicitly, so each needs one added line:

```rust
                SchemaSnapshotEntry::Column {
                    native_id: _,
                    parent_schema,
                    parent_table,
                    name,
                    data_type,
                    nullable,
                    ordinal,
                    default,
                } => {
```

```rust
                SchemaSnapshotEntry::Index {
                    native_id: _,
                    parent_schema,
                    parent_table,
                    name,
                    columns,
                    unique,
                    index_type,
                } => {
```

Identity still comes from `SchemaObjectId::new()` in this stage; stage 2 replaces it. Nothing else in the service changes.

- [ ] **Step 6: Run the test to verify it passes**

```bash
cargo test --workspace -- --ignored pg_snapshot_entries_carry_stable_native_ids
```

Expected: PASS. Then confirm nothing else regressed:

```bash
cargo test --workspace -- --ignored
```

Expected: the 15 pre-existing integration tests still pass. `pg_schema_refresh` asserts only counts and versions, so the query rewrite does not change it.

- [ ] **Step 7: Verify the whole workspace is green**

```bash
cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace
```

Expected: no output from fmt, no warnings, all tests pass.

- [ ] **Step 8: Commit**

```bash
git add crates/tempr_db/src/driver.rs crates/tempr_db_postgres/src/driver.rs crates/tempr_services/src/schema.rs crates/tempr/tests/integration.rs
git commit -m "feat(db): introspect via pg_catalog and carry native ids on schema entries"
```

---

## Task 2: `SchemaScope::SearchPath`

**Files:**
- Modify: `crates/tempr_db/src/driver.rs` (the `SchemaScope` enum, ~line 13)
- Modify: `crates/tempr_db_postgres/src/driver.rs` (`scope_clause`)
- Test: `crates/tempr/tests/integration.rs`

**Interfaces:**
- Consumes: `scope_clause` and the catalog queries from Task 1
- Produces: `SchemaScope::SearchPath` — introspection limited to the schemas on `current_schemas(false)` plus `public`. Stage 2's `SchemaService` uses this as its default scope.

- [ ] **Step 1: Write the failing test**

```rust
#[tokio::test]
#[ignore = "requires DATABASE_URL env var pointing to a live PostgreSQL instance"]
async fn pg_search_path_scope_excludes_off_path_schemas() {
    let (_bus, cs) = setup_pg_cs();
    let id = connect_test_pg(&cs).await;

    cs.with_metadata_connection_fn(id, |mut conn| async move {
        conn.execute("DROP SCHEMA IF EXISTS off_path CASCADE", &[]).await?;
        conn.execute("CREATE SCHEMA off_path", &[]).await?;
        conn.execute("CREATE TABLE off_path.hidden (id int)", &[]).await?;
        conn.execute("DROP TABLE IF EXISTS on_path_probe", &[]).await?;
        conn.execute("CREATE TABLE on_path_probe (id int)", &[]).await
    })
    .await
    .expect("setup schemas");

    let scoped = cs
        .with_metadata_connection_fn(id, |mut conn| async move {
            conn.snapshot_schema(SchemaScope::SearchPath).await
        })
        .await
        .expect("search-path snapshot");
    let all = cs
        .with_metadata_connection_fn(id, |mut conn| async move {
            conn.snapshot_schema(SchemaScope::All).await
        })
        .await
        .expect("all snapshot");

    let has = |entries: &[SchemaSnapshotEntry], want_schema: &str, want_name: &str| {
        entries.iter().any(|e| match e {
            SchemaSnapshotEntry::Table { schema, name, .. } => {
                schema == want_schema && name == want_name
            }
            _ => false,
        })
    };

    assert!(has(&scoped, "public", "on_path_probe"), "public must be in scope");
    assert!(!has(&scoped, "off_path", "hidden"), "off-path schema must be excluded");
    assert!(has(&all, "off_path", "hidden"), "All scope must still see it");

    cs.with_metadata_connection_fn(id, |mut conn| async move {
        conn.execute("DROP SCHEMA off_path CASCADE", &[]).await?;
        conn.execute("DROP TABLE on_path_probe", &[]).await
    })
    .await
    .expect("cleanup");
}
```

- [ ] **Step 2: Run the test to verify it fails**

```bash
cargo test --workspace -- --ignored pg_search_path_scope_excludes_off_path_schemas
```

Expected: compile error — no variant `SearchPath` on `SchemaScope`.

- [ ] **Step 3: Add the variant**

In `crates/tempr_db/src/driver.rs`:

```rust
/// Scope for schema introspection.
#[derive(Debug, Clone)]
pub enum SchemaScope {
    /// Schemas the connection can reference unqualified — its `search_path`
    /// plus `public`. The default for catalog refreshes: it matches what
    /// unqualified SQL can actually name, and keeps large multi-tenant
    /// databases from loading schemas nobody in this session will reference.
    SearchPath,
    /// Every non-system schema.
    All,
    Schema(String),
    Table { schema: String, table: String },
}
```

- [ ] **Step 4: Implement the clause**

In `scope_clause` in `crates/tempr_db_postgres/src/driver.rs`, add the arm:

```rust
        SchemaScope::SearchPath => (
            format!("({ns}.nspname = ANY (current_schemas(false)) OR {ns}.nspname = 'public')"),
            Vec::new(),
        ),
```

`current_schemas(false)` excludes the implicit `pg_catalog`, which the catalog does not want as user-visible content.

- [ ] **Step 5: Run the test to verify it passes**

```bash
cargo test --workspace -- --ignored pg_search_path_scope_excludes_off_path_schemas
```

Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/tempr_db/src/driver.rs crates/tempr_db_postgres/src/driver.rs crates/tempr/tests/integration.rs
git commit -m "feat(db): SchemaScope::SearchPath for search_path + public introspection"
```

---

## Task 3: Function entries

**Files:**
- Modify: `crates/tempr_db/src/driver.rs` (`SchemaSnapshotEntry`)
- Modify: `crates/tempr_db_postgres/src/driver.rs` (`snapshot_schema`)
- Modify: `crates/tempr_services/src/schema.rs` (handle the new variant)
- Test: `crates/tempr/tests/integration.rs`

**Interfaces:**
- Consumes: `scope_clause`, `as_params`, the entry enum from Tasks 1-2
- Produces: `SchemaSnapshotEntry::Function { native_id, schema, name, parameters: Vec<(String, String)>, return_type, language }`, where `parameters` pairs an argument name (or `$1`, `$2`… when the function declares none) with its formatted type. Completion in stage 5 renders these.

- [ ] **Step 1: Write the failing test**

```rust
#[tokio::test]
#[ignore = "requires DATABASE_URL env var pointing to a live PostgreSQL instance"]
async fn pg_snapshot_includes_functions() {
    let (_bus, cs) = setup_pg_cs();
    let id = connect_test_pg(&cs).await;

    cs.with_metadata_connection_fn(id, |mut conn| async move {
        conn.execute("DROP FUNCTION IF EXISTS add_two(integer, integer)", &[]).await?;
        conn.execute(
            "CREATE FUNCTION add_two(a integer, b integer) RETURNS integer \
             LANGUAGE sql AS $$ SELECT a + b $$",
            &[],
        )
        .await
    })
    .await
    .expect("setup function");

    let entries = cs
        .with_metadata_connection_fn(id, |mut conn| async move {
            conn.snapshot_schema(SchemaScope::SearchPath).await
        })
        .await
        .expect("snapshot");

    let func = entries
        .iter()
        .find_map(|e| match e {
            SchemaSnapshotEntry::Function { name, .. } if name == "add_two" => Some(e.clone()),
            _ => None,
        })
        .expect("add_two missing from snapshot");

    match func {
        SchemaSnapshotEntry::Function {
            native_id,
            schema,
            parameters,
            return_type,
            language,
            ..
        } => {
            assert_ne!(native_id, 0);
            assert_eq!(schema, "public");
            assert_eq!(
                parameters,
                vec![
                    ("a".to_string(), "integer".to_string()),
                    ("b".to_string(), "integer".to_string()),
                ]
            );
            assert_eq!(return_type, "integer");
            assert_eq!(language, "sql");
        }
        other => panic!("expected a Function entry, got {other:?}"),
    }

    cs.with_metadata_connection_fn(id, |mut conn| async move {
        conn.execute("DROP FUNCTION add_two(integer, integer)", &[]).await
    })
    .await
    .expect("cleanup");
}
```

- [ ] **Step 2: Run the test to verify it fails**

```bash
cargo test --workspace -- --ignored pg_snapshot_includes_functions
```

Expected: compile error — no `Function` variant.

- [ ] **Step 3: Add the variant**

Append to `SchemaSnapshotEntry` in `crates/tempr_db/src/driver.rs`:

```rust
    Function {
        native_id: u64,
        schema: String,
        name: String,
        /// `(argument name, formatted type)`; unnamed arguments are `$1`, `$2`, …
        parameters: Vec<(String, String)>,
        return_type: String,
        language: String,
    },
```

- [ ] **Step 4: Query functions in `snapshot_schema`**

First add a function-specific scope helper next to `scope_clause` — `pg_proc` has no `relname`, and scoping functions by a *table* name is meaningless, so `SchemaScope::Table` narrows to that table's schema:

```rust
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
```

Then add before `Ok(entries)` in `snapshot_schema`:

```rust
        // Plain functions only: 'p' is a procedure, 'a' an aggregate, 'w' a window
        // function — none of which complete like a scalar call.
        let (fn_where, fn_binds) = function_scope_clause(&scope, "n");
        let fn_params = as_params(&fn_binds);
        let sql = format!(
            "SELECT p.oid::int8, n.nspname, p.proname, \
                    COALESCE(p.proargnames, ARRAY[]::text[]), \
                    ARRAY(SELECT format_type(t, NULL) FROM unnest(p.proargtypes) AS t), \
                    format_type(p.prorettype, NULL), l.lanname \
             FROM pg_proc p \
             JOIN pg_namespace n ON n.oid = p.pronamespace \
             JOIN pg_language l ON l.oid = p.prolang \
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
```

- [ ] **Step 5: Handle the variant in `SchemaService`**

`crates/tempr_services/src/schema.rs` matches exhaustively over entries. Add an arm that maps it to the domain's existing `SchemaObject::Function`:

```rust
                SchemaSnapshotEntry::Function {
                    native_id: _,
                    schema,
                    name,
                    parameters,
                    return_type,
                    language,
                } => {
                    objects.push(SchemaObject::Function {
                        id: SchemaObjectId::new(),
                        schema: schema.clone(),
                        name: name.clone(),
                        parameters: parameters.clone(),
                        return_type: return_type.clone(),
                        language: language.clone(),
                    });
                }
```

- [ ] **Step 6: Run the test to verify it passes**

```bash
cargo test --workspace -- --ignored pg_snapshot_includes_functions
```

Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add crates/tempr_db/src/driver.rs crates/tempr_db_postgres/src/driver.rs crates/tempr_services/src/schema.rs crates/tempr/tests/integration.rs
git commit -m "feat(db): introspect functions into schema snapshots"
```

---

## Task 4: Fingerprint sweep

**Files:**
- Modify: `crates/tempr_db/src/error.rs` (new `Unsupported` variant)
- Modify: `crates/tempr_db/src/driver.rs` (`SchemaFingerprint`, `ObjectKind`, trait method)
- Modify: `crates/tempr_db/src/lib.rs` (re-exports)
- Modify: `crates/tempr_db_postgres/src/driver.rs` (implementation)
- Test: `crates/tempr/tests/integration.rs`

**Interfaces:**
- Consumes: `scope_clause`, `as_params`
- Produces:
  ```rust
  pub struct SchemaFingerprint { pub native_id: u64, pub kind: ObjectKind, pub version: u64 }
  pub enum ObjectKind { Relation, Column }
  async fn schema_fingerprints(&mut self, scope: SchemaScope)
      -> Result<Vec<SchemaFingerprint>, DriverError>;   // default: Err(DriverError::Unsupported)
  ```
  Stage 2 diffs two fingerprint lists to decide what to re-introspect.

- [ ] **Step 1: Write the failing test**

```rust
#[tokio::test]
#[ignore = "requires DATABASE_URL env var pointing to a live PostgreSQL instance"]
async fn pg_fingerprints_move_only_for_changed_relations() {
    let (_bus, cs) = setup_pg_cs();
    let id = connect_test_pg(&cs).await;

    cs.with_metadata_connection_fn(id, |mut conn| async move {
        conn.execute("DROP TABLE IF EXISTS fp_touched", &[]).await?;
        conn.execute("DROP TABLE IF EXISTS fp_untouched", &[]).await?;
        conn.execute("CREATE TABLE fp_touched (id int)", &[]).await?;
        conn.execute("CREATE TABLE fp_untouched (id int)", &[]).await
    })
    .await
    .expect("setup tables");

    let before = cs
        .with_metadata_connection_fn(id, |mut conn| async move {
            conn.schema_fingerprints(SchemaScope::SearchPath).await
        })
        .await
        .expect("first fingerprints");

    cs.with_metadata_connection_fn(id, |mut conn| async move {
        conn.execute("ALTER TABLE fp_touched ADD COLUMN label text", &[]).await
    })
    .await
    .expect("alter table");

    let after = cs
        .with_metadata_connection_fn(id, |mut conn| async move {
            conn.schema_fingerprints(SchemaScope::SearchPath).await
        })
        .await
        .expect("second fingerprints");

    // Map the probe tables to their relation OIDs via a snapshot.
    let entries = cs
        .with_metadata_connection_fn(id, |mut conn| async move {
            conn.snapshot_schema(SchemaScope::SearchPath).await
        })
        .await
        .expect("snapshot");
    let oid_of = |want: &str| -> u64 {
        entries
            .iter()
            .find_map(|e| match e {
                SchemaSnapshotEntry::Table { native_id, name, .. } if name == want => {
                    Some(*native_id)
                }
                _ => None,
            })
            .unwrap_or_else(|| panic!("{want} missing from snapshot"))
    };
    let version_of = |fps: &[SchemaFingerprint], oid: u64| -> u64 {
        fps.iter()
            .find(|f| f.native_id == oid && f.kind == ObjectKind::Relation)
            .map(|f| f.version)
            .expect("relation fingerprint missing")
    };

    let touched = oid_of("fp_touched");
    let untouched = oid_of("fp_untouched");
    assert!(!before.is_empty(), "fingerprint sweep returned nothing");
    assert_ne!(
        version_of(&before, touched),
        version_of(&after, touched),
        "altered relation must change its fingerprint"
    );
    assert_eq!(
        version_of(&before, untouched),
        version_of(&after, untouched),
        "untouched relation must keep its fingerprint"
    );

    cs.with_metadata_connection_fn(id, |mut conn| async move {
        conn.execute("DROP TABLE fp_touched", &[]).await?;
        conn.execute("DROP TABLE fp_untouched", &[]).await
    })
    .await
    .expect("cleanup");
}
```

Extend the test file's import to:

```rust
use tempr_db::{ObjectKind, SchemaFingerprint, SchemaScope, SchemaSnapshotEntry};
```

- [ ] **Step 2: Run the test to verify it fails**

```bash
cargo test --workspace -- --ignored pg_fingerprints_move_only_for_changed_relations
```

Expected: compile error — `ObjectKind`, `SchemaFingerprint` and `schema_fingerprints` do not exist.

- [ ] **Step 3: Add the error variant**

In `crates/tempr_db/src/error.rs`, inside `DriverError`:

```rust
    #[error("unsupported by this driver: {0}")]
    Unsupported(String),
```

- [ ] **Step 4: Add the types and the trait method**

In `crates/tempr_db/src/driver.rs`:

```rust
/// What a fingerprint refers to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ObjectKind {
    Relation,
    Column,
}

/// A cheap change marker for one schema object. `version` changes whenever the
/// object's definition changes; comparing two sweeps yields the set of objects
/// worth re-introspecting. PostgreSQL uses the catalog row's `xmin`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchemaFingerprint {
    pub native_id: u64,
    pub kind: ObjectKind,
    pub version: u64,
}
```

And on `DriverConnection`, after `snapshot_schema`:

```rust
    /// Cheap change-detection sweep over `scope`: one row per relation and per
    /// column, carrying a version that moves when the object's definition
    /// changes. Callers diff two sweeps and re-introspect only what moved.
    ///
    /// Drivers that cannot do this return `DriverError::Unsupported`, and the
    /// caller falls back to a full introspection.
    async fn schema_fingerprints(
        &mut self,
        scope: SchemaScope,
    ) -> Result<Vec<SchemaFingerprint>, DriverError> {
        let _ = scope;
        Err(DriverError::Unsupported(
            "schema_fingerprints".to_string(),
        ))
    }
```

Export from `crates/tempr_db/src/lib.rs`:

```rust
pub use driver::{
    CancelHandle, DatabaseDriver, DriverConnection, EngineId, ObjectKind, SchemaFingerprint,
    SchemaScope, SchemaSnapshotEntry,
};
```

- [ ] **Step 5: Implement it for PostgreSQL**

In `crates/tempr_db_postgres/src/driver.rs`, inside `impl DriverConnection for PostgresConnection`:

```rust
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
             WHERE c.relkind IN ('r', 'p', 'v', 'm') AND {where_clause} \
             UNION ALL \
             SELECT (a.attrelid::int8 << 16) | a.attnum::int8, 1::int2, a.xmin::text::int8 \
             FROM pg_attribute a \
             JOIN pg_class c ON c.oid = a.attrelid \
             JOIN pg_namespace n ON n.oid = c.relnamespace \
             WHERE a.attnum > 0 AND NOT a.attisdropped \
               AND c.relkind IN ('r', 'p', 'v', 'm') AND {where_clause}"
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
```

And the small helper it needs, next to `scope_clause`:

```rust
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
```

- [ ] **Step 6: Write a unit test for the placeholder helper**

In `crates/tempr_db_postgres/src/driver.rs`, in its `#[cfg(test)] mod tests`:

```rust
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
```

- [ ] **Step 7: Run the tests**

```bash
cargo test -p tempr_db_postgres
cargo test --workspace -- --ignored pg_fingerprints_move_only_for_changed_relations
```

Expected: both PASS.

- [ ] **Step 8: Commit**

```bash
git add crates/tempr_db/src/driver.rs crates/tempr_db/src/error.rs crates/tempr_db/src/lib.rs crates/tempr_db_postgres/src/driver.rs crates/tempr/tests/integration.rs
git commit -m "feat(db): schema fingerprint sweep for incremental refresh"
```

---

## Task 5: Server keyword list

**Files:**
- Modify: `crates/tempr_db/src/driver.rs` (trait method)
- Modify: `crates/tempr_db_postgres/src/driver.rs` (implementation)
- Test: `crates/tempr/tests/integration.rs`

**Interfaces:**
- Consumes: nothing from earlier tasks
- Produces: `async fn keywords(&mut self) -> Result<Vec<String>, DriverError>` — default `Ok(Vec::new())`. Stage 2 stores the result on `SchemaSnapshot`; stage 5 offers them as completion candidates.

- [ ] **Step 1: Write the failing test**

```rust
#[tokio::test]
#[ignore = "requires DATABASE_URL env var pointing to a live PostgreSQL instance"]
async fn pg_keywords_come_from_the_server() {
    let (_bus, cs) = setup_pg_cs();
    let id = connect_test_pg(&cs).await;

    let words = cs
        .with_metadata_connection_fn(id, |mut conn| async move { conn.keywords().await })
        .await
        .expect("keywords");

    assert!(words.len() > 100, "expected a full keyword list, got {}", words.len());
    assert!(words.iter().any(|w| w == "select"), "keywords are lower-cased");
    assert!(words.iter().any(|w| w == "join"));
}
```

- [ ] **Step 2: Run the test to verify it fails**

```bash
cargo test --workspace -- --ignored pg_keywords_come_from_the_server
```

Expected: compile error — no method `keywords`.

- [ ] **Step 3: Add the trait method**

In `crates/tempr_db/src/driver.rs`, on `DriverConnection`:

```rust
    /// The engine's own keyword list, fetched once per schema refresh and
    /// cached with the catalog — never on the completion request path.
    /// Drivers with no such list return an empty vector.
    async fn keywords(&mut self) -> Result<Vec<String>, DriverError> {
        Ok(Vec::new())
    }
```

- [ ] **Step 4: Implement it for PostgreSQL**

```rust
    async fn keywords(&mut self) -> Result<Vec<String>, DriverError> {
        let rows = self
            .client
            .query("SELECT word FROM pg_get_keywords()", &[])
            .await
            .map_err(|e| DriverError::Query(e.to_string()))?;
        Ok(rows.into_iter().map(|row| row.get(0)).collect())
    }
```

`pg_get_keywords()` returns words lower-cased, which is what the completion ranker wants; casing at render time is stage 5's problem.

- [ ] **Step 5: Run the test to verify it passes**

```bash
cargo test --workspace -- --ignored pg_keywords_come_from_the_server
```

Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/tempr_db/src/driver.rs crates/tempr_db_postgres/src/driver.rs crates/tempr/tests/integration.rs
git commit -m "feat(db): expose the server keyword list on DriverConnection"
```

---

## Task 6: Documentation and pull request

**Files:**
- Modify: `docs/09-database-engine.md`
- Modify: `docs/PROGRESS.md`
- Modify: `docs/TODO.md`

**Interfaces:**
- Consumes: everything from Tasks 1-5
- Produces: the merged stage 1 that stage 2 builds on

- [ ] **Step 1: Document the driver additions**

In `docs/09-database-engine.md`, in the driver-trait section, document: `native_id` on every entry and the rule for engines without a stable id; the `Function` entry; `SchemaScope::SearchPath` and its `current_schemas(false)` + `public` definition; `schema_fingerprints` with its `xmin` basis and the `Unsupported` fallback; `keywords`. Keep it to the contract — the SQL lives in the code.

- [ ] **Step 2: Update the living docs**

`docs/PROGRESS.md`: status block (last completed / verified / next action), a session-log row for today, and a decisions-log row pointing at the spec. Phase 3's checklist boxes stay unchecked — no criterion is met until stage 2 caches something.

`docs/TODO.md`: add the two items this stage defers — a per-connection setting for the default catalog scope, and `estimated_rows` coming from `reltuples` (which is `-1` until the relation is analyzed, so it is absent on fresh databases rather than zero).

- [ ] **Step 3: Full verification**

```bash
docker start tempr-pg
export DATABASE_URL='postgres://tempr:tempr@localhost:55432/tempr?sslmode=disable'
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo test --workspace -- --ignored
```

Expected: fmt silent, clippy clean, unit tests pass, and every integration test passes — the 15 that existed plus the 5 added here. If the TLS tests fail, the TLS container needs recreating (see the recipe in the PROGRESS decisions log); that is environmental, not a code failure.

- [ ] **Step 4: Commit and open the PR**

```bash
git add docs/
git commit -m "docs: driver identity additions for Phase 3 stage 1"
git push -u origin feat/ph3-driver-identity
```

Then run `/code-review`, fix what it finds, and open the PR with `gh pr create`, describing: the pg_catalog rewrite and why (OIDs), the new trait methods and their defaults, the `SearchPath` scope, and the verification output above.

---

## Notes for the implementer

**Why the introspection queries changed.** `pg_tables` and `information_schema` expose no OIDs, and OIDs are the whole point of this stage. The rewrite also changes one observable detail: `data_type` now comes from `format_type(atttypid, atttypmod)`, so a `varchar(50)` column reports `character varying(50)` where `information_schema` reported `character varying`. That is more precise and is what hover will want. No existing test asserts the old string.

**Why fingerprints include columns.** `ALTER TABLE ... ALTER COLUMN ... TYPE` rewrites the `pg_attribute` row but may leave `pg_class` alone. Sweeping both means a column type change is never missed.

**What this stage deliberately does not do.** No caching, no identity derivation, no service behaviour change. `SchemaService` still mints random `SchemaObjectId`s and still returns the same snapshot shape to its callers. If a test in this stage asserts anything about `SchemaService` behaviour beyond "it still compiles and still passes", it has drifted out of scope.
