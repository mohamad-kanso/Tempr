# Tempr — Progress

> **Single source of truth for where the build stands.** Update after every
> completed task per the living-docs rule in CLAUDE.md. Phase definitions
> live in docs/16-roadmap.md; business scope in docs/PRODUCT.md.

## Current status

- **Last completed**: Phase 1 UI — `Input` (ported from gpui `examples/input.rs`) + `ResultsGrid` (`uniform_list`) + status bar in `MainWindow`; `QueryService::execute_streaming` + `RowSink` stream batches to the grid over a channel with `RowsReceived` per batch; bus→UI `UiEvent` bridge; connection from `DATABASE_URL` (2026-09-03, branch `feat/ph1-input-grid`)
- **Verified**: `cargo fmt --all --check` ✅ · `cargo clippy --workspace --all-targets -- -D warnings` ✅ · `cargo test --workspace` ✅ (76 unit) · `cargo test -p tempr --test integration -- --ignored` ✅ (10 tests vs Docker PG 16, incl. `pg_execute_streaming_100k_rows_in_batches`) · `cargo deny check` ✅ · app launched against Docker PG: window opens, `ConnectionStateChanged` Connecting→Connected; wrong password → Connecting→Failed (status bar path) (2026-09-03)
- **Next action**: Measure grid scroll frame time at 100k rows (needs a frame-time probe or screenshot tooling); then cancel-query keybinding + connection picker replacing `DATABASE_URL`; core services implement `Service`; deadpool wiring

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
- [x] `tempr_db_postgres` crate: `PostgresDriver`, `PostgresConnection`, `PostgresStream` (pinned `Pin<Box<RowStream>>`), `decode_value` (all PG types: bool, int, float, numeric, text, uuid, json, timestamp, date, time) *(6 tests, 2026-07-14)*
- [x] Extended `Value` with Uuid, Json, Timestamp, Date, Time, Numeric, Array, Custom; added `ValueType`, `ColumnSpec`, `Batch` to tempr_domain *(16 tests, 2026-07-14)*
- [x] `ConnectionService`: pool management, state tracking, `with_connection_fn` exclusive-access pattern, event publishing, proper Failed state on missing driver *(8 tests, 2026-07-14)*
- [x] `QueryService`: execute → stream → finalize lifecycle, `completed_runs` storage, event publishing *(4 tests, 2026-07-14)*
- [x] `SchemaService`: two-pass schema introspection (tables → columns/indexes via parent ID map), snapshot versioning, event publishing *(3 tests, 2026-07-14)*
- [x] Binary wires all services + PostgreSQL driver *(compiled, 2026-07-14)*
- [x] `ServiceError` extended with `QueryFailed`, `ConnectionNotFound`, `NotConnected` *(5 registry tests, 2026-07-14)*
- [x] 9 integration tests written (ignored, require DATABASE_URL): connect, insert/select, decode mixed types, insert returning, streaming, auth failure, schema refresh, syntax error, event publishing *(2026-07-14)*
- [x] SQL injection fixed in `snapshot_schema` — parameterized queries (`$1`, `$2`) *(2026-07-14)*
- [x] Decode layer handles PostgreSQL text-format booleans (`t`/`f`/`true`/`false`) and timestamps with timezone variants *(2026-07-14)*
- [x] GPUI dependency lands: `rust-toolchain.toml` 1.97.1, `gpui`+`gpui_platform`+`gpui_tokio` at one rev with `cargo deny` license gate, `tempr_ui` crate + `gpui_compat` shim, binary opens a 1200×800 window and starts services on the tokio bridge *(verified via `xwininfo`, 2026-09-03, → D17)*
- [ ] PostgreSQL driver connects over TLS using Phase 0 connection config
- [x] GPUI window renders with text input and scrollable result grid *(`Input` + `ResultsGrid` + status bar; builds, opens against Docker PG; 2026-09-03 — pixel output not screenshot-verified: GNOME denies CLI screenshots)*
- [x] Connection/auth/syntax errors produce user-visible messages via event system *(status bar: `ConnectionStateChanged{Failed}` via `UiEvent` bridge — verified with wrong password; query errors via `execute_streaming` result + `QueryFinished{Error}`; syntax-error path covered by `pg_query_syntax_error`; 2026-09-03)*
- [ ] Result grid displays streaming rows; smooth scroll for up to 100,000 rows *(streaming half done: 100k rows in batches through `RowSink` → channel → `uniform_list`, verified by integration test 2026-09-03; 60 fps scroll unmeasured)*

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
| 2026-08-17 | GPUI dependency = `gpui` + `gpui_platform` at one rev; Apache-2.0 Zed crates only (no GPL `ui`/`theme`/`markdown`/`editor`); tokio↔GPUI bridge required | → D16 |
| 2026-09-03 | GPUI rev pinned to Zed `main` HEAD `ed8d600` (2026-09-03), not a release tag — every tag ≤ v1.18.0 carries GPL `zlog`/`ztracing` under gpui | → D17 |
| 2026-09-03 | `deny.toml` allows Zlib, CC0-1.0, bzip2-1.0.6, NCSA (permissive, pulled by gpui's graph); `cargo deny check licenses` is the D16 gate | → D17 |
| 2026-09-03 | `gpui_tokio` adopted as the tokio↔GPUI bridge (Apache-2.0 verified) | → D17 |
| 2026-09-03 | `rust-toolchain.toml` pins `1.97.1`, minimal profile + rustfmt/clippy; CI installs it with `rustup show` (no action input duplicates the pin) | → D17 |
| 2026-09-03 | `.cargo/config.toml` sets `net.git-fetch-with-cli = true` | libgit2 fetched 3 MB of the Zed repo in 10 min; system git completes the clone |
| 2026-09-03 | `QueryService::execute_streaming(sql, id, Arc<dyn RowSink>)` is the UI query path; `execute` collects via the same `run()` and stays for tests/scripting; `RowsReceived` published per batch | Service never holds the rows the grid displays (memory pillar, 13-result-grid); one code path, two sinks |
| 2026-09-03 | Bus→UI via `tempr_ui::events::bridge` mapping `AppEvent` → `Clone` `UiEvent` over a futures channel drained with `cx.spawn` | `AppEvent` cannot be `Clone` (`PluginPayload` is `Box<dyn Any>`); views need owned events on the main thread |
| 2026-09-03 | Phase 1 grid stores `Vec<Vec<Value>>`; the columnar `RowStore` + spill from 13-result-grid.md is deferred | 100k rows × few columns is well under the 500 MB NFR; virtualization (the render-side risk) is in place now |
| 2026-09-03 | Connection comes from `DATABASE_URL` until the workspace connection list exists | Same source the integration tests use; no UI for secrets yet |
| 2026-09-03 | GPUI code lives in new `tempr_ui` crate (per 14-project-layout.md); binary depends on it; all `gpui`/`gpui_platform`/`gpui_tokio` calls go through `tempr_ui::gpui_compat` | Isolates upstream churn to one module (11-gpui.md shim rule) |

## Session log

| Date | Phase | What was done | Follow-ups |
|---|---|---|---|
| 2026-07-13 | Phase 0 | Architecture suite (16 docs + 9 ADRs + RFC) written; Cargo workspace + 5 crates scaffolded; domain types (15 tests), event bus (6), service registry (5), workspace manifest (4), storage (6) implemented; CI workflow + cargo-deny configured; MIT license set (→ D12); all 8 exit criteria verified; PR review workflow (D13) adopted: pre-push hook + setup script + CLAUDE.md hard rule; OD#5 resolved: upstream git pin, no fork (→ D14); lorekeeper check: zero drift; D13 judgment exception eliminated — no direct commits to main ever (→ D15) | Begin Phase 1; run bash scripts/setup.sh in each new worktree |
| 2026-07-14 | Phase 1 | Created `feat/phase1-db-layer` branch; implemented database layer: `tempr_db` driver traits crate, `tempr_db_postgres` PostgreSQL driver (tokio-postgres, batched streaming, PG type decode), extended `Value` enum (8 new variants + `ValueType`/`ColumnSpec`/`Batch`), `ConnectionService` (pool + `with_connection_fn` exclusive access), `QueryService` (execute → stream → event lifecycle), `SchemaService` (PG introspection + snapshot), binary wiring; fixed compilation: DriverConnection `Send + Sync`, PostgresStream pinning, service API redesign; 37 tests pass, clippy clean, fmt clean | Wire deadpool-postgres; add PG integration tests; GPUI application shell |
| 2026-07-14 | Phase 1 | Fixed 5 failing tests (bool decode: `t`/`f` format, timestamp timezone offset `+00` handling), fixed ConnectionService missing `Failed` state on no-driver path, added `ServiceError::{QueryFailed, ConnectionNotFound, NotConnected}`, added 7 integration tests (PG connect/select/insert/streaming/auth/schema/events), parameterized schema snapshot queries (SQL injection fix), pinned PostgresStream (`Pin<Box<RowStream>>`); **65 unit tests pass** — clippy clean, fmt clean, cargo deny clean | Run integration tests with DATABASE_URL; begin GPUI shell |
| 2026-07-14 | Phase 1 | Fixed Docker PostgreSQL compatibility: switched from `query_raw` to `client.query()` (lifetime issue with RowStream borrowing client), added `password` field to `Connection` struct, switched `sslmode=require` to `sslmode=disable`, added `PostgresStream::from_rows` for collected results; **all 7 integration tests pass** against Docker PostgreSQL | Commit and merge; begin GPUI shell |
| 2026-08-17 | Phase 1 | Branch `docs/gpui-verified-api`: folded the verified GPUI API survey (local `zed-industries/zed` clone, gpui `0.2.2`) into docs/11-gpui.md — corrected `Render` trait + 2-arg `render(&mut Window, &mut Context<Self>)` signature, `Entity<T>`/`cx.new` state model, `gpui_platform::application()` entry point, dependency wiring (two crates, one rev), what GPUI actually ships (no text input, no 2D grid, no usable theme), `uniform_list`/`list`+`ListState` virtualization APIs, GPUI executor vs tokio (`gpui_tokio` bridge), Zed crate licensing (GPL `ui`/`theme`/`markdown`/`editor` out of bounds); resolved 2 open questions; **D16** recorded | Verify `gpui_tokio` license; add `rust-toolchain.toml` ≥ 1.95.0; PR |
| 2026-09-03 | Phase 1 | Branch `feat/ph1-gpui-deps`: `rust-toolchain.toml` (1.97.1); `gpui`/`gpui_platform`(features wayland,x11)/`gpui_tokio` pinned at Zed `main` `ed8d600`; `cargo deny` caught GPL `zlog`/`ztracing`/`ztracing_macro` at tag v1.9.0 → re-pinned past upstream relicense `ac5af8b9` (→ D17); deny allowlist +Zlib/CC0-1.0/bzip2-1.0.6/NCSA, 3 unmaintained advisories ignored with reasons, yanked `chacha20` bumped; `.cargo/config.toml` git-CLI fetch; CI installs Linux gpui libs + reads toolchain file; new `tempr_ui` crate (`gpui_compat`: `run_app`/`open_main_window`/`spawn_tokio`; `MainWindow` placeholder view); binary rewritten: sync `main`, services built pre-GPUI, started via `Tokio::spawn`; fmt/clippy/test/deny green; window verified 1200×800; **PR #4 merged**. Then branch `feat/ph1-input-grid`: `RowSink` + `QueryService::execute_streaming` (+2 tests), `tempr_ui::components::{Input, ResultsGrid}`, `events::bridge` (`UiEvent`), `MainWindow` wiring (connect on start, Enter/ctrl-enter runs, rows stream per batch, status bar), `DATABASE_URL` connection in binary, `value_format` (+4 tests); 76 unit + 10 PG integration tests (new `pg_execute_streaming_100k_rows_in_batches`) against Docker PG 16 on :55432; dev libs installed by user (no `LIBRARY_PATH` needed) | Scroll frame-time measurement at 100k rows; cancel keybinding; connection picker; core services → `Service` trait; deadpool wiring |
| 2026-07-14 | Phase 1 | Code review fix pass (10 findings): fixed typed column decoding (finding #1), params passthrough (#2), RETURNING rows (#3), batch-size chunking (#4), schema error propagation (#5), schema scope for columns/indexes (#6), index columns via pg_index (#7), cancel handle capture (#8), configurable sslmode (#9), conninfo escaping via Config builder (#10); reverted #4 from `query_raw` to `query()`+chunked batch due to live-DB `Closed` error with `query_raw` through QueryService (true lazy streaming deferred to TODO); cleaned up debug pollution from root-cause investigation | Lazy wire streaming as follow-up; commit and PR |
