# Tempr — Progress

> **Single source of truth for where the build stands.** Update after every
> completed task per the living-docs rule in CLAUDE.md. Phase definitions
> live in docs/16-roadmap.md; business scope in docs/PRODUCT.md.

## Current status

- **Last completed**: Phase 1 database layer — all checks pass, all 87 unit tests green (2026-07-14)
- **Verified**: `cargo fmt --all` ✅ · `cargo clippy --all-targets -- -D warnings` ✅ · `cargo test --workspace` ✅ (87 tests: 16 domain, 6 events, 20 services, 10 workspace, 7 db, 28 postgres) + 7 integration tests (ignored, require DATABASE_URL)
- **Next action**: Wire deadpool-postgres connection pool; begin GPUI application shell

## Phase checklist

*Reconstructed from git history on 2026-07-13. Two commits total: initial project scaffold + full documentation suite. No implementation code yet.*

### Phase 0 — Foundations ✅ complete (2026-07-13)
- [x] Cargo workspace builds with zero warnings on Linux, macOS, Windows *(cargo clippy clean, 2026-07-13)*
- [x] Domain types (`Workspace`, `Connection`, `Query`, `Result`) compile and pass unit tests *(15 tests, 2026-07-13)*
- [x] Event bus dispatches typed events to registered handlers; ordering and delivery verified by tests *(6 tests, 2026-07-13)*
- [x] Service registry supports registration, lookup, and lifecycle (start/stop) with mock services *(5 tests, 2026-07-13)*
- [x] Workspace file format is read/write round-trip safe; malformed files produce structured errors *(4 manifest tests, 2026-07-13)*
- [x] Storage layer writes and reads structured data to platform-specific data directory *(6 storage tests, 2026-07-13)*
- [x] CI pipeline runs `cargo fmt --check`, `cargo clippy`, `cargo test`, `cargo deny` on every push *(.github/workflows/ci.yml, 2026-07-13)*
- [x] All 16 architecture docs exist and cross-reference correctly *(verified 2026-07-13)*

### Phase 1 — Connect & Run
- [x] `tempr_db` crate: `DatabaseDriver`, `DriverConnection` traits, `QueryStream`, `EngineId`, `SchemaScope`, `SchemaSnapshotEntry`, `DriverError` *(7 tests, 2026-07-14)*
- [x] `tempr_db_postgres` crate: `PostgresDriver`, `PostgresConnection`, `PostgresStream` (pinned `Pin<Box<RowStream>>`), `decode_value` (all PG types: bool, int, float, numeric, text, uuid, json, timestamp, date, time) *(28 tests, 2026-07-14)*
- [x] Extended `Value` with Uuid, Json, Timestamp, Date, Time, Numeric, Array, Custom; added `ValueType`, `ColumnSpec`, `Batch` to tempr_domain *(16 tests, 2026-07-14)*
- [x] `ConnectionService`: pool management, state tracking, `with_connection_fn` exclusive-access pattern, event publishing, proper Failed state on missing driver *(8 tests, 2026-07-14)*
- [x] `QueryService`: execute → stream → finalize lifecycle, `completed_runs` storage, event publishing *(4 tests, 2026-07-14)*
- [x] `SchemaService`: two-pass schema introspection (tables → columns/indexes via parent ID map), snapshot versioning, event publishing *(3 tests, 2026-07-14)*
- [x] Binary wires all services + PostgreSQL driver *(compiled, 2026-07-14)*
- [x] `ServiceError` extended with `QueryFailed`, `ConnectionNotFound`, `NotConnected` *(5 registry tests, 2026-07-14)*
- [x] 7 integration tests written (ignored, require DATABASE_URL): connect, insert/select, streaming, auth failure, schema refresh, syntax error, event publishing *(2026-07-14)*
- [x] SQL injection fixed in `snapshot_schema` — parameterized queries (`$1`, `$2`) *(2026-07-14)*
- [x] Decode layer handles PostgreSQL text-format booleans (`t`/`f`/`true`/`false`) and timestamps with timezone variants *(2026-07-14)*
- [ ] PostgreSQL driver connects over TLS using Phase 0 connection config
- [ ] GPUI window renders with text input and scrollable result grid
- [ ] Result grid displays streaming rows; smooth scroll for up to 100,000 rows
- [ ] Connection/auth/syntax errors produce user-visible messages via event system

### Phase 2 — Editor
- [ ] Rope buffer handles 10 MB documents with sub-millisecond insert/delete
- [ ] Tree-sitter PostgreSQL grammar produces incremental syntax tree
- [ ] Statement detector identifies boundaries ($$, comments, string literals)
- [ ] Command palette: opens via keybinding, fuzzy search, executes commands
- [ ] Keybindings configurable via workspace format; default map provided
- [ ] Full editing ops on rope buffer (cursor, selection, copy/paste, undo/redo, line ops)
- [ ] "Execute statement under cursor" works end-to-end
- [ ] Keyboard-only audit complete: every action listed with its keybinding

### Phase 3 — Intelligence
- [ ] Catalog cache loads full schema metadata from PostgreSQL and caches locally
- [ ] Cache refreshes incrementally; full refresh available on demand
- [ ] Completion provider: context-aware suggestions ranked by relevance
- [ ] Completion latency < 5 ms for 10,000 cached schema objects
- [ ] Semantic analyzer resolves column refs; detects ambiguous/unresolvable names
- [ ] Diagnostics for syntax (tree-sitter) and semantic errors in real time
- [ ] Hover shows column types and table definitions
- [ ] All intelligence features work offline after initial schema load; no I/O on request path

### Phase 4 — Extensibility & Polish
- [ ] Plugin API stable: versioned, documented, backward-compatible within major versions
- [ ] Core features migrated to plugin API as reference implementations
- [ ] Theme system: light and dark themes, configurable accent colors
- [ ] Query history persisted and browsable
- [ ] Layout state persists across restarts
- [ ] Third-party-style plugin adds a panel and completion provider via public API only
- [ ] Platform-native bundles: .deb, .rpm, AppImage, .dmg, MSI

## Decisions log

> Fine-grained session decisions. MAJOR decisions (architecture, business
> rules, deviations, user directives) live in **docs/DECISIONS.md** — new
> rows for those link `→ D<n>` instead of restating the rationale.

| Date | Decision | Why |
|---|---|---|
| 2026-07-13 | GPUI selected as sole UI framework | → D1 |
| 2026-07-13 | MIT license chosen as working default; `license.workspace = true` in all crates; OD#2 closed | → D12 |
| 2026-07-13 | Rust-only constraint locked | → D2 |
| 2026-07-13 | Custom SQL editor, no embedded editors | → D3 |
| 2026-07-13 | PostgreSQL first via driver abstraction | → D4 |
| 2026-07-13 | Workspace-first scope model | → D5 |
| 2026-07-13 | Service-oriented architecture | → D6 |
| 2026-07-13 | Internal event bus for service communication | → D7 |
| 2026-07-13 | Plugin-extensible from day one | → D8 |
| 2026-07-13 | Internal semantic engine, not LSP | → D9 |
| 2026-07-13 | Capability-gated roadmap phases over date-based milestones | → D10 |
| 2026-07-13 | Lorekeeper living-docs adopted | → D11 |
| 2026-07-13 | PR review-based workflow adopted — branch → PR → /code-review → user approval | → D13 |
| 2026-07-13 | GPUI dependency strategy: upstream git pin, no fork; OD#5 closed | → D14 |
| 2026-07-13 | No direct commits to main ever — judgment exception in D13 eliminated | → D15 |

## Session log

| Date | Phase | What was done | Follow-ups |
|---|---|---|---|
| 2026-07-13 | Phase 0 | Architecture suite (16 docs + 9 ADRs + RFC) written; Cargo workspace + 5 crates scaffolded; domain types (15 tests), event bus (6), service registry (5), workspace manifest (4), storage (6) implemented; CI workflow + cargo-deny configured; MIT license set (→ D12); all 8 exit criteria verified; PR review workflow (D13) adopted: pre-push hook + setup script + CLAUDE.md hard rule; OD#5 resolved: upstream git pin, no fork (→ D14); lorekeeper check: zero drift; D13 judgment exception eliminated — no direct commits to main ever (→ D15) | Begin Phase 1; run bash scripts/setup.sh in each new worktree |
| 2026-07-14 | Phase 1 | Created `feat/phase1-db-layer` branch; implemented database layer: `tempr_db` driver traits crate, `tempr_db_postgres` PostgreSQL driver (tokio-postgres, batched streaming, PG type decode), extended `Value` enum (8 new variants + `ValueType`/`ColumnSpec`/`Batch`), `ConnectionService` (pool + `with_connection_fn` exclusive access), `QueryService` (execute → stream → event lifecycle), `SchemaService` (PG introspection + snapshot), binary wiring; fixed compilation: DriverConnection `Send + Sync`, PostgresStream pinning, service API redesign; 37 tests pass, clippy clean, fmt clean | Wire deadpool-postgres; add PG integration tests; GPUI application shell |
| 2026-07-14 | Phase 1 | Fixed 5 failing tests (bool decode: `t`/`f` format, timestamp timezone offset `+00` handling), fixed ConnectionService missing `Failed` state on no-driver path, added `ServiceError::{QueryFailed, ConnectionNotFound, NotConnected}`, added 7 integration tests (PG connect/select/insert/streaming/auth/schema/events), parameterized schema snapshot queries (SQL injection fix), pinned PostgresStream (`Pin<Box<RowStream>>`); **87 tests pass** — clippy clean, fmt clean, cargo deny clean | Run integration tests with DATABASE_URL; begin GPUI shell |
