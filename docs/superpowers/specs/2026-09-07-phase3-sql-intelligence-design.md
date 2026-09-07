# Phase 3 — SQL Intelligence: design

**Date**: 2026-09-07
**Status**: approved in brainstorming, pending implementation plans
**Phase**: 3 (Intelligence) — all eight exit criteria in [16-roadmap.md](../../16-roadmap.md)
**Governing docs**: [12-sql-intelligence.md](../../12-sql-intelligence.md), [07-storage.md](../../07-storage.md), [05-services.md](../../05-services.md), [09-database-engine.md](../../09-database-engine.md), [14-project-layout.md](../../14-project-layout.md)
**Locked decisions honoured**: D9 (internal engine, no LSP, no DB queries during completion), D6 (no business logic in the UI), D21/D22 (buffer and syntax tree), D23 (command catalog), D24 (workspace layering)

---

## 1. Scope

This design covers the whole of Phase 3: the catalog cache, the semantic engine, completion, diagnostics, and hover. It is written to be implemented as six sequenced pull requests (§9), each independently reviewable.

**In scope**

- Stable schema-object identity derived from the database engine's own ids.
- A persisted catalog cache under `.tempr/cache/catalog/`, plus full and incremental refresh.
- An in-memory catalog optimized for name resolution and prefix search.
- A semantic engine resolving scopes, aliases, CTEs and subqueries, producing diagnostics.
- Context-aware completion under a 5 ms budget, with ranking.
- Editor integration: completion popup, diagnostic underlines, hover, and the commands that drive them.

**Out of scope**

- Expression type inference and type-mismatch diagnostics (described in 12-sql-intelligence, demanded by no Phase 3 criterion; deferred to TODO).
- Cross-file workspace symbol indexing (`WorkspaceIndex` in 12-sql-intelligence) — there is no file-open flow yet; the engine is written so the index can be added as a second candidate source without reshaping the API.
- Plugin-authored completion providers beyond the call seam (no plugins ship in this phase).
- Schema browser UI.

---

## 2. Decisions taken in brainstorming

| # | Decision | Rationale |
|---|---|---|
| 1 | Phase 3 is specified as one design, implemented as six PRs | User's call; keeps the phase's criteria visible in one place while staying reviewable |
| 2 | Object identity derives from the PostgreSQL OID | Stable across refresh, restart and rename; exact diffing; free from the catalog query |
| 3 | Incremental refresh detects change with a fingerprint sweep over `(oid, xmin)` | No privileges beyond catalog reads, no objects written into the user's database, and a real delta rather than a disguised full re-fetch |
| 4 | Cache format is bincode behind a versioned header | Serde is already in the graph; a version mismatch discards derived data safely. Closes OD#1 in 07-storage |
| 5 | Completion opens automatically after one identifier character, after a qualifier dot, or on ctrl-space | User's call; matches the roadmap demo |
| 6 | The analyzer resolves scopes, aliases, CTEs and subqueries — no type inference | Exactly what the exit criteria require |
| 7 | Completion and hover are synchronous on the UI thread; diagnostics are analyzed on a debounce in the background | The only arrangement that satisfies both "< 5 ms" and "never block the UI thread" |

Decisions 2, 3, 4 and 7 are MAJOR and get DECISIONS.md entries when their PR lands (D25–D28), not before — an entry describes a decision that shipped.

---

## 3. Object identity

### Problem

`SchemaService::refresh` mints `SchemaObjectId::new()` (a fresh UUID) for every object on every refresh. Nothing survives a reload, so no cache can be diffed, and hover or go-to-definition has no durable target.

### Design

`SchemaSnapshotEntry` (in `tempr_db`) grows a `native_id: u64` on every variant, and a `Function` variant is added:

```rust
pub enum SchemaSnapshotEntry {
    Table    { native_id: u64, schema: String, name: String, estimated_rows: Option<u64> },
    View     { native_id: u64, schema: String, name: String, definition: String },
    Column   { native_id: u64, parent_schema: String, parent_table: String, name: String,
               data_type: String, nullable: bool, ordinal: usize, default: Option<String> },
    Index    { native_id: u64, parent_schema: String, parent_table: String, name: String,
               columns: Vec<String>, unique: bool, index_type: String },
    Function { native_id: u64, schema: String, name: String,
               parameters: Vec<(String, String)>, return_type: String, language: String },
}
```

PostgreSQL supplies: `pg_class.oid` for tables, views and indexes; `pg_proc.oid` for functions; `(attrelid as u64) << 16 | attnum` for columns, which is unique because `attnum` is an `int2`.

`SchemaObjectId` is then derived, not random:

```rust
impl SchemaObjectId {
    pub fn derived(connection: ConnectionId, native_id: u64) -> Self;
}
```

The existing `SchemaObjectId::new()` stays for tests and for drivers with no stable id.

**Consequences.** The driver trait is engine-specific in one narrow way: a driver must produce *some* stable `u64`. A future engine without one hashes its qualified name into the same field, and the catalog behaves as it does today (renames read as delete + insert). This is written into the trait's documentation so the next driver author does not have to guess.

---

## 4. Catalog cache

### 4.1 In-memory form (`tempr_intel::CatalogCache`)

`SchemaSnapshot`'s flat `Vec<SchemaObject>` remains the wire and disk shape. The engine holds a resolution-shaped structure built once per snapshot:

```rust
pub struct CatalogCache {
    objects: Vec<CatalogObject>,                          // arena; other maps hold u32 indices
    by_id: HashMap<SchemaObjectId, u32>,
    relations_by_name: HashMap<(Box<str>, Box<str>), u32>,          // (schema, name), case-sensitive
    relations_by_folded_name: HashMap<(Box<str>, Box<str>), Vec<u32>>,
    columns_of: HashMap<u32, Range<u32>>,                 // columns stored contiguously per relation
    name_order: Vec<u32>,                                 // case-folded sort of relation + function names
    schemas: Vec<Box<str>>,
    stats: CatalogStats,                                  // counts, built_at, source snapshot id
}
```

Std types only — no new dependency. Interning or small-string crates are an optimization to reach for if the load probe in §8 says so, and would need a DECISIONS entry under the D18 rule.

- Columns are laid out contiguously per relation, so "columns of this table" is a slice, not a scan.
- `name_order` supports prefix search by binary search plus a scan of the matching run.
- Unquoted SQL identifiers fold to lower case (PostgreSQL's rule); quoted identifiers use the case-sensitive map. Both maps point into the same arena.
- The whole structure is immutable once built. A refresh builds a new one and swaps the `Arc` under a `parking_lot::RwLock<Arc<CatalogCache>>` (already a dependency); a reader takes the read lock only long enough to clone the `Arc`, then works lock-free against an immutable snapshot. Readers never see a half-updated catalog, and a refresh never stalls a completion.

### 4.2 On-disk form

Path: `<workspace>/.tempr/cache/catalog/<connection_id>.tcat`

```
magic        "TCAT"      4 bytes
format       u16         bumped on any layout change
flags        u16         reserved, zero
snapshot_id  16 bytes
connection   16 bytes
fetched_at   i64         unix millis
content_hash u64         hash of the serialized snapshot body
body         bincode(SchemaSnapshot)
```

Load: read the header; on unknown magic, a `format` this build does not understand, a truncated body, or a `content_hash` mismatch, the file is **discarded** and the catalog is re-introspected. The cache is derived data, so discarding is always safe and never surfaces an error to the user beyond a log line.

Write: through `Storage`, atomically (temp file + rename), and only when `content_hash` differs from the file already there — an unchanged schema never rewrites.

`Storage` gains, per 07-storage:

```rust
async fn catalog_cache(&self, connection: ConnectionId) -> Result<Box<dyn CatalogCacheFile>, WorkspaceError>;

pub trait CatalogCacheFile: Send + Sync {
    async fn load(&self) -> Result<Option<SchemaSnapshot>, WorkspaceError>;
    async fn save(&self, snapshot: &SchemaSnapshot) -> Result<(), WorkspaceError>;
}
```

Until workspace open lands, the workspace root is the one D24 already resolves (`TEMPR_WORKSPACE`, else the current directory); with no writable root, the catalog stays in memory only and logs that it is not persisting.

### 4.3 Refresh

Two paths on `SchemaService`:

**Full refresh** — today's introspection, extended with native ids and functions. Runs on first connect when no cache file exists, on `schema::Refresh`, and whenever an incremental refresh reports more than 40 % of relations changed (at that point a full sweep is cheaper than many targeted queries).

**Incremental refresh** — a new driver method:

```rust
async fn schema_fingerprints(&mut self, scope: SchemaScope)
    -> Result<Vec<SchemaFingerprint>, DriverError>;

pub struct SchemaFingerprint { pub native_id: u64, pub kind: ObjectKind, pub version: u64 }
```

PostgreSQL implements it as a single query over `pg_class` and `pg_attribute` returning `oid` and `xmin::text::bigint`. `xmin` is the transaction that last wrote that catalog row, so any DDL moves it.

The diff is then:

- fingerprint present, `version` changed → re-introspect that relation (and its columns and indexes)
- fingerprint absent, object cached → delete from the catalog
- fingerprint present, object not cached → introspect it
- otherwise → keep the cached object untouched

A frozen catalog row (after `VACUUM FREEZE`) reports `xmin = 2`, which differs from the cached value and triggers a re-introspect. That is a false positive, never a missed change — the safe direction.

Both paths publish `SchemaRefreshed { connection, snapshot }` exactly as today, so existing subscribers are unaffected. `IntelligenceService` subscribes and swaps the `CatalogCache` behind the engine's lock.

---

## 5. Semantic engine (`tempr_intel::SemanticEngine`)

```rust
pub struct SemanticEngine {
    catalog: RwLock<Arc<CatalogCache>>,
    analyses: RwLock<HashMap<SqlFileId, CachedAnalysis>>,  // keyed by buffer edit id
}

impl SemanticEngine {
    pub fn analyze(&self, file: SqlFileId, buffer: &Buffer) -> Arc<Analysis>;
    pub fn complete(&self, req: &CompletionRequest<'_>) -> Vec<CompletionItem>;
    pub fn hover(&self, file: SqlFileId, buffer: &Buffer, offset: usize) -> Option<HoverInfo>;
    pub fn set_catalog(&self, catalog: Arc<CatalogCache>);
}
```

`Buffer` already exposes monotonic edit ids that survive undo/redo (`edit_ids_increase_and_survive_undo_redo`), so a cached analysis is validated by an integer compare. A stale entry is dropped, never trusted.

### 5.1 Analysis pipeline

Per statement (statement ranges come from `SyntaxTree::statement_ranges`, already built in Phase 2):

1. **Collect sources.** Walk `FROM` and `JOIN` clauses into bindings:
   ```rust
   struct Binding { alias: Option<Box<str>>, name: Box<str>, source: BindingSource }
   enum BindingSource { Relation(u32), Cte(CteId), Subquery(ScopeId) }
   ```
2. **Build the scope tree.** `WITH` registers CTE bindings visible to later CTEs and to the main body; every subquery gets a child scope that sees its own sources plus its ancestors (correlated references).
3. **Resolve references.**
   - unqualified `col` → search visible bindings in scope order; 0 matches → `UnknownColumn`; >1 → `AmbiguousColumn` naming both candidates
   - qualified `a.col` → resolve `a` as alias, then relation name, then schema; unresolved → `UnknownRelation`
   - `*` and `a.*` expand to the ordered column lists of the bindings in scope
4. **Emit diagnostics.**
   ```rust
   pub enum DiagnosticKind { SyntaxError, UnknownRelation, UnknownColumn, AmbiguousColumn }
   pub struct Diagnostic { pub range: Range<usize>, pub kind: DiagnosticKind, pub message: String }
   ```
   Syntax diagnostics are lifted from the tree's `Error` nodes (`StatementKind::Error` exists already).

A statement whose parse contains an error still resolves what it can; a diagnostic never suppresses completion, because the buffer is mid-edit almost every time completion runs.

**Unknown catalog guard.** When no catalog is loaded (no connection yet, or introspection failed), the analyzer emits *no* unknown/ambiguous diagnostics — it cannot distinguish a typo from an unloaded schema, and false errors on every identifier would be worse than silence. Syntax diagnostics still show.

### 5.2 Completion

```rust
pub struct CompletionRequest<'a> { pub file: SqlFileId, pub buffer: &'a Buffer,
                                   pub offset: usize, pub explicit: bool }
pub struct CompletionItem { pub label: String, pub kind: CompletionKind, pub detail: String,
                            pub insert: String, pub score: i32 }
```

Context classification from the cursor's node:

| Position | Candidates |
|---|---|
| after `FROM` / `JOIN` / `UPDATE` / `INTO` | relations, then schema names |
| after a qualifier dot | columns of that binding, or relations of that schema |
| expression position | columns in scope (weighted up), relations, functions, keywords |
| statement start | statement keywords |

Ranking is one score: context weight, then match quality (exact prefix > case-folded prefix > subsequence, reusing the palette's `fuzzy_match`), then shorter label; ties broken by label so output is stable. Candidates land in a bounded top-N heap (N = 200) so a 10 000-object catalog never materializes a full list.

Hot path rules: no I/O; the only lock is the read guard held for the `Arc` clone; no allocation of catalog data — candidates borrow from the arena and only the returned items own their strings.

### 5.3 Hover

`hover` resolves the reference under the offset through the same scope machinery and returns the column's type, nullability, default and owning relation, or a relation's column list. It reuses the cached analysis, so hover after completion is a map lookup.

---

## 6. Service and UI integration

**`IntelligenceService`** (new, per 05-services) owns the engine, subscribes to `SchemaRefreshed` (swap catalog) and `BufferChanged` (schedule analysis). Analysis runs on a ~100 ms idle debounce on a background task, re-analyzing only statements whose ranges intersect the edit, then publishes a new event:

```rust
AppEvent::DiagnosticsReady { file: SqlFileId }
```

`BufferChanged` exists in the taxonomy but has no publisher — `EditorView` starts publishing it after each edit batch, which also closes a TODO from Phase 2.

**`tempr_ui`**

- `CompletionPopup` — caret-anchored overlay, `uniform_list` of items (label, kind, detail). Deliberately **not** `Modal`: typing keeps flowing into the buffer and re-filters the list. A `Completion` key context layered on `Editor` binds only navigation, accept and dismiss.
- Diagnostics render as underlines through the existing highlight-run machinery, with a count in the status bar. The message for the diagnostic at the cursor appears in the same popup surface hover uses.
- Hover is keyboard-first; a mouse-hover trigger can follow later without changing the engine.

**Commands** (all keyed — the audit test fails otherwise):

| Command | Default |
|---|---|
| `editor::TriggerCompletion` | ctrl-space |
| `completion::Next` / `completion::Prev` | down / up (plus ctrl-n / ctrl-p) |
| `completion::Accept` | enter, tab |
| `completion::Dismiss` | escape |
| `editor::Hover` | ctrl-k ctrl-i |
| `editor::NextDiagnostic` / `editor::PrevDiagnostic` | f8 / shift-f8 |
| `schema::Refresh` | ctrl-shift-r |
| `schema::RefreshIncremental` | ctrl-alt-r |

Completion triggers automatically after one identifier character in a completable position and immediately after a qualifier dot; ctrl-space forces it anywhere; escape dismisses.

**Binding conflicts are resolved by context depth, deliberately.** `enter`, `tab` and `escape` are already bound in the `Editor` context (`editor::Newline`, `editor::Tab`) and in the window (`main_window::CancelQuery` under `MainWindow && !Modal`). The `Completion` context is added to the editor's key context *only while the popup is open*, and GPUI dispatches from the innermost context outward, so those three keys mean accept / accept / dismiss while completing and go back to their editor meanings the moment the popup closes. This is the same mechanism the palette uses, minus `Modal` — the popup must not suspend typing. The audit test gains a case asserting that no `Completion` binding leaks when the popup is closed.

---

## 7. Error handling

| Situation | Behaviour |
|---|---|
| No connection / no catalog | Completion offers keywords only; analyzer emits syntax diagnostics only |
| Cache file missing, truncated, wrong version, hash mismatch | Discard, log, full introspect |
| Cache directory unwritable | Catalog stays in memory; one log line; no user-facing error |
| Introspection fails (permissions, dropped connection) | Keep the previous catalog; surface the failure in the status bar; features keep working from cache (the offline criterion) |
| Fingerprint query unsupported by a driver | Default trait method returns `Unsupported`; `SchemaService` falls back to full refresh |
| Grammar produces an `Error` node around the cursor | Completion still runs from the nearest resolvable scope; no semantic diagnostics for that statement |

---

## 8. Testing

**Unit**
- Catalog: construction from a snapshot, name lookups (folded and quoted), `columns_of` slicing, prefix search boundaries.
- Fingerprint diff against a fake driver: added, changed, dropped, renamed, unchanged; the >40 % full-refresh fallback.
- `.tcat`: round-trip, version mismatch discarded, truncated body discarded, unchanged snapshot does not rewrite.
- Analyzer: a table of SQL string → expected diagnostics, covering aliases, CTEs, nested and correlated subqueries, set operations, `*` expansion, ambiguity between two joined tables, unknown relation, and the no-catalog silence rule.
- Completion: a table of (SQL, cursor offset) → expected head of the ranked list, one row per context in §5.2.

**Integration (Docker PostgreSQL)**
- Introspection carries real oids; functions appear.
- Incremental refresh after `CREATE TABLE` / `ALTER TABLE ADD COLUMN` / `DROP TABLE` re-fetches exactly the touched relations and leaves the rest untouched (asserted by object identity, not by count alone).
- Offline criterion: introspect, stop the container, rebuild the engine from `.tcat`, and complete successfully.

**Performance** (`#[ignore]`d, release, in the style of the buffer probes)
- Completion p99 < 5 ms over a synthetic 10 000-object catalog, reported with p50/p95/p99/max.
- Catalog load from `.tcat` for the same catalog, reported so OD#1's format choice can be revisited against a number rather than an opinion.

---

## 9. Implementation stages

Each stage is one PR: green `fmt`, `clippy -D warnings`, tests, and its own docs updates.

| # | Stage | Delivers | Depends on |
|---|---|---|---|
| 1 | Driver identity | `native_id` on entries, `Function` entries, `schema_fingerprints`, PG implementation + integration tests | — |
| 2 | Catalog persistence | `Storage::catalog_cache`, `.tcat` format, full + incremental refresh in `SchemaService`, `SchemaObjectId::derived` | 1 |
| 3 | `tempr_intel` crate | In-memory `CatalogCache`, lookups, load probe | 2 |
| 4 | Semantic analysis | Scopes, aliases, CTEs, subqueries, diagnostics model | 3 |
| 5 | Completion | Context classification, ranking, 5 ms probe | 4 |
| 6 | UI | `BufferChanged` publisher, `IntelligenceService`, popup, underlines, hover, commands | 5 |

**Grammar probe before stage 4 is planned.** Everything in §5.1 assumes `tree-sitter-sequel` exposes usable nodes for `WITH`, subqueries and join clauses. Phase 2 only ever asked it for statement boundaries and highlight captures. The probe parses a handful of real queries (CTE chains, correlated subqueries, joins with aliases, set operations) and dumps node kinds; if the grammar turns out too shallow, stage 4's scope extraction changes shape — and that is worth knowing before the plan is written, not during it.

---

## 10. Open questions

1. **Keyword list source.** Completion needs SQL keywords. Options: the grammar's own keyword set, a hand-maintained list, or `pg_get_keywords()` from the connected server (a query — but at refresh time, never on the request path). Leaning on the last, cached with the catalog, because it matches the connected server's version exactly.
2. **Catalog scope.** Whether to introspect and cache every schema in the database or only those on `search_path` plus `public`. Full introspection is simpler and the 10 000-object target assumes it; a database with hundreds of tenant schemas would argue otherwise. Decide when stage 2 has a real timing.
3. **Diagnostic severity.** All four kinds are currently errors. `AmbiguousColumn` may deserve warning status once the analyzer is real.

---

## 11. Documentation impact

Landing these stages updates: `PROGRESS.md` (Phase 3 checklist, status, session log), `PRODUCT.md` (section 4 markers), `TODO.md` (deferred items above; the `BufferChanged` publisher row closes), `DECISIONS.md` (D25–D28 as their PRs land), `07-storage.md` (OD#1 resolved, `catalog_cache` signature), `09-database-engine.md` (driver trait additions), `12-sql-intelligence.md` (reconcile the sketched API with what ships), `05-services.md` (`IntelligenceService` signatures), `11-gpui.md` (command catalog table, popup component), and `14-project-layout.md` (`tempr_intel` exists).
