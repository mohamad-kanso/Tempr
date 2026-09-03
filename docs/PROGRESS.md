# Tempr — Progress

> **Single source of truth for where the build stands.** Update after every
> completed task per the living-docs rule in CLAUDE.md. Phase definitions
> live in docs/16-roadmap.md; business scope in docs/PRODUCT.md.

## Current status

- **Last completed**: Phase 2 boxes 2 + 3 — `SyntaxTree` (`tree-sitter-sequel` on `tree-sitter` 0.25) owned by `Buffer`; edits feed `InputEdit`s, parse runs lazily on read; statement ranges from the tree (`;` folded, comments skipped, `$$` bodies whole); bundled highlights query. Release probes: rope edit 5.9 µs avg; incremental reparse 1.6 ms (10 MB, 2.5k statements) / 150 ms (10 MB, 180k statements) vs 2–2.7 s full parse (→ D22; 2026-09-03, branch `feat/ph2-syntax-tree`)
- **Verified**: `cargo fmt --all --check` ✅ · `cargo clippy --workspace --all-targets -- -D warnings` ✅ · `cargo test --workspace` ✅ · `cargo test -p tempr_editor --release -- --ignored` ✅ (10 MB probes above) · `cargo deny check` ✅ (2026-09-03)
- **Next action**: Phase 2 box 4/5 — command palette + configurable keybindings (Command registry in `tempr_services`, palette view in `tempr_ui`), then editing operations on the buffer (cursor/selection/clipboard/line ops) and "execute statement under cursor" wiring `Buffer::statement_at` to `QueryService`

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
- [x] PostgreSQL driver connects over TLS using Phase 0 connection config *(rustls, `TlsMode` on `Connection`; 5 TLS integration tests against a self-signed `ssl=on` PostgreSQL 16 — require/prefer encrypt, disable plaintext, verify-full rejects unknown CA; 2026-09-03, → D20)*
- [x] GPUI window renders with text input and scrollable result grid *(`Input` + `ResultGrid` + status bar; builds, opens against Docker PG; 2026-09-03; rendering confirmed by user screenshot: input, typed column headers, rows, status bar "Done — 100000 rows")*
- [x] Connection/auth/syntax errors produce user-visible messages *(status bar + grid pane: connection failures arrive as `ConnectionStateChanged{Failed}` through the `UiEvent` bridge — verified with wrong password; query errors arrive through the `execute_streaming` task result (the `QueryFinished{Error}` bus event is published too but the view does not consume it); syntax-error path covered by `pg_query_syntax_error`; 2026-09-03)*
- [x] Result grid displays streaming rows; smooth scroll for up to 100,000 rows *(streaming: `RowSink` → channel → `uniform_list`, integration test + user screenshot; scroll: `DebugScrollBenchmark` release, display on, user-run — 57.2 fps, avg 17.5 ms, p95 23.1 ms, max 59 ms, 61/603 frames > 20 ms under a worst-case 166-row jump per frame; 10% slow frames tracked in TODO; 2026-09-03)*

### Phase 2 — Editor
- [x] Rope buffer handles 10 MB documents with sub-millisecond insert/delete *(`tempr_editor::Buffer`; release probe on a 10 MB buffer: avg 1.73 µs, worst 12.1 µs per insert+delete; 9 unit tests; 2026-09-03, → D21)*
- [x] Tree-sitter PostgreSQL grammar produces incremental syntax tree *(`tree-sitter-sequel` 0.3 / `tree-sitter` 0.25; `Buffer` feeds `InputEdit`s and re-parses lazily; incremental == full parse in tests; release: 1.6 ms reparse on 10 MB realistic SQL, 150 ms on a 180k-statement dump; 2026-09-03, → D22)*
- [x] Statement detector identifies boundaries ($$, comments, string literals) *(derived from the tree-sitter tree: `SyntaxTree::statement_ranges`/`statement_at`; test covers `;` inside string literals, line and block comments, and `$tag$…$tag$` bodies; 2026-09-03, → D22)*
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

> Older rows (Phase 0 – early Phase 1, 2026-07-13 … 2026-09-03) are archived in [archive/PROGRESS-2026.md](archive/PROGRESS-2026.md).

| Date | Decision | Why |
|---|---|---|
| 2026-09-03 | New utility deps `unicode-segmentation`, `percent-encoding` (both MIT/Apache) | → D18 |
| 2026-09-03 | Scroll performance is measured in-app: `DebugScrollBenchmark` command (ctrl-shift-b) drives `UniformListScrollHandle::scroll_to_item_strict` once per `on_next_frame`, reports fps/p95/max/dropped (> 20 ms); `TEMPR_BENCH_SCROLL=1` + `TEMPR_STARTUP_SQL` make it headless and exit; bench mode disables gpui's inactive-window 30 Hz cap | No screenshot tooling here; a frame-time probe is the only way to verify the 60 fps AC, and it must be a keyboard `Command` per the no-mouse-only rule. Caveat learned: the display must be on — a blanked compositor delivers frames at ~1 Hz |
| 2026-09-03 | Dev knobs are read from env in the binary and passed as `tempr_ui::DevOptions` — the UI crate never touches the environment | Keeps `tempr_ui` config-driven and testable |
| 2026-09-03 | `QueryService::execute_streaming_with_id(run_id, …)` lets the view allocate the `QueryRunId` up front; `cancel` of a not-yet-registered run is armed (`pending_cancels`) and applied when it starts; a cancel landing after registration but before the driver handle is captured aborts before `execute`; cancelling a completed run is a no-op with no event | Review: escape right after Enter used to be silently ignored and an unknown-run cancel published a spurious `QueryFinished{Cancelled}` |
| 2026-09-03 | `MainWindow.current_run: Option<QueryRunId>` is the single owner of "a query is running"; only the outcome task clears it; `QueryFinished` bus events no longer touch view state | Review: the bus `Cancelled` arm raced the outcome task and allowed a second concurrent run |
| 2026-09-03 | `ConnectionService` pools via `deadpool` core over the driver trait; separate 1-slot metadata pool for `SchemaService`; `connect` warms one connection; `Service::stop` drains pools, `QueryService::stop` cancels runs; `stop_all` runs from `on_app_quit` | → D19 |
| 2026-09-03 | `DriverConnection::is_closed` added to the driver trait (sync, no I/O) so the pool evicts dead idle connections on recycle | Review: a dropped socket was re-pooled forever while the UI stayed "Connected" |
| 2026-09-03 | PostgreSQL TLS via rustls + platform roots; `TlsMode` mirrors libpq `sslmode`, default `prefer`; `verify-ca` ≡ `verify-full` | → D20 |
| 2026-09-03 | TLS integration tests run against a second container: `docker run -d --name tempr-pg-tls -e POSTGRES_PASSWORD=tempr -e POSTGRES_USER=tempr -e POSTGRES_DB=tempr -p 55433:5432 -v <dir>/server.crt:/certs/server.crt:ro -v <dir>/server.key:/certs/server.key.src:ro postgres:16-alpine sh -c 'cp /certs/server.key.src /tmp/server.key && chown postgres:postgres /tmp/server.key && chmod 600 /tmp/server.key && exec docker-entrypoint.sh postgres -c ssl=on -c ssl_cert_file=/certs/server.crt -c ssl_key_file=/tmp/server.key'` with a self-signed cert from `openssl req -x509 -newkey rsa:2048 -nodes -days 30 -subj /CN=localhost -addext subjectAltName=DNS:localhost,IP:127.0.0.1`; `DATABASE_URL_TLS=postgres://tempr:tempr@localhost:55433/tempr` | Reproducible local TLS target without touching the plain container |
| 2026-09-03 | `tempr_editor::Buffer`: byte-offset API over `ropey`, `edit` returns `Result`, buffer is a pure model (owner publishes `BufferChanged`), latency criterion checked by an ignored release test | → D21 |
| 2026-09-03 | Syntax tree re-parses lazily on read, not inside `edit`; statement detection is derived from the tree (no hand-written scanner); highlights from the grammar's bundled query | → D22 |
| 2026-09-03 | GPUI code lives in new `tempr_ui` crate (per 14-project-layout.md); binary depends on it; all `gpui`/`gpui_platform`/`gpui_tokio` calls go through `tempr_ui::gpui_compat` | Isolates upstream churn to one module (11-gpui.md shim rule) |

## Session log

| Date | Phase | What was done | Follow-ups |
|---|---|---|---|
| 2026-07-13 | Phase 0 | Architecture suite (16 docs + 9 ADRs + RFC) written; Cargo workspace + 5 crates scaffolded; domain types (15 tests), event bus (6), service registry (5), workspace manifest (4), storage (6) implemented; CI workflow + cargo-deny configured; MIT license set (→ D12); all 8 exit criteria verified; PR review workflow (D13) adopted: pre-push hook + setup script + CLAUDE.md hard rule; OD#5 resolved: upstream git pin, no fork (→ D14); lorekeeper check: zero drift; D13 judgment exception eliminated — no direct commits to main ever (→ D15) | Begin Phase 1; run bash scripts/setup.sh in each new worktree |
| 2026-07-14 | Phase 1 | Created `feat/phase1-db-layer` branch; implemented database layer: `tempr_db` driver traits crate, `tempr_db_postgres` PostgreSQL driver (tokio-postgres, batched streaming, PG type decode), extended `Value` enum (8 new variants + `ValueType`/`ColumnSpec`/`Batch`), `ConnectionService` (pool + `with_connection_fn` exclusive access), `QueryService` (execute → stream → event lifecycle), `SchemaService` (PG introspection + snapshot), binary wiring; fixed compilation: DriverConnection `Send + Sync`, PostgresStream pinning, service API redesign; 37 tests pass, clippy clean, fmt clean | Wire deadpool-postgres; add PG integration tests; GPUI application shell |
| 2026-07-14 | Phase 1 | Fixed 5 failing tests (bool decode: `t`/`f` format, timestamp timezone offset `+00` handling), fixed ConnectionService missing `Failed` state on no-driver path, added `ServiceError::{QueryFailed, ConnectionNotFound, NotConnected}`, added 7 integration tests (PG connect/select/insert/streaming/auth/schema/events), parameterized schema snapshot queries (SQL injection fix), pinned PostgresStream (`Pin<Box<RowStream>>`); **65 unit tests pass** — clippy clean, fmt clean, cargo deny clean | Run integration tests with DATABASE_URL; begin GPUI shell |
| 2026-07-14 | Phase 1 | Fixed Docker PostgreSQL compatibility: switched from `query_raw` to `client.query()` (lifetime issue with RowStream borrowing client), added `password` field to `Connection` struct, switched `sslmode=require` to `sslmode=disable`, added `PostgresStream::from_rows` for collected results; **all 7 integration tests pass** against Docker PostgreSQL | Commit and merge; begin GPUI shell |
| 2026-08-17 | Phase 1 | Branch `docs/gpui-verified-api`: folded the verified GPUI API survey (local `zed-industries/zed` clone, gpui `0.2.2`) into docs/11-gpui.md — corrected `Render` trait + 2-arg `render(&mut Window, &mut Context<Self>)` signature, `Entity<T>`/`cx.new` state model, `gpui_platform::application()` entry point, dependency wiring (two crates, one rev), what GPUI actually ships (no text input, no 2D grid, no usable theme), `uniform_list`/`list`+`ListState` virtualization APIs, GPUI executor vs tokio (`gpui_tokio` bridge), Zed crate licensing (GPL `ui`/`theme`/`markdown`/`editor` out of bounds); resolved 2 open questions; **D16** recorded | Verify `gpui_tokio` license; add `rust-toolchain.toml` ≥ 1.95.0; PR |
| 2026-09-03 | Phase 1 | Branch `feat/ph1-gpui-deps`: `rust-toolchain.toml` (1.97.1); `gpui`/`gpui_platform`(features wayland,x11)/`gpui_tokio` pinned at Zed `main` `ed8d600`; `cargo deny` caught GPL `zlog`/`ztracing`/`ztracing_macro` at tag v1.9.0 → re-pinned past upstream relicense `ac5af8b9` (→ D17); deny allowlist +Zlib/CC0-1.0/bzip2-1.0.6/NCSA, 3 unmaintained advisories ignored with reasons, yanked `chacha20` bumped; `.cargo/config.toml` git-CLI fetch; CI installs Linux gpui libs + reads toolchain file; new `tempr_ui` crate (`gpui_compat`: `run_app`/`open_main_window`/`spawn_tokio`; `MainWindow` placeholder view); binary rewritten: sync `main`, services built pre-GPUI, started via `Tokio::spawn`; fmt/clippy/test/deny green; window verified 1200×800; **PR #4 merged**. Then branch `feat/ph1-input-grid`: `RowSink` + `QueryService::execute_streaming` (+2 tests), `tempr_ui::components::{Input, ResultGrid}`, `events::bridge` (`UiEvent`), `MainWindow` wiring (connect on start, Enter/ctrl-enter runs, rows stream per batch, status bar), `DATABASE_URL` connection in binary, `value_format` (+4 tests); 76 unit + 10 PG integration tests (new `pg_execute_streaming_100k_rows_in_batches`) against Docker PG 16 on :55432; dev libs installed by user (no `LIBRARY_PATH` needed); **PR #5 merged** (user screenshot verified rendering). Then branch `feat/ph1-grid-perf-cancel`: `ScrollBench` (+3 tests), `DebugScrollBenchmark`/`CancelQuery` actions, `DevOptions` (`TEMPR_STARTUP_SQL`, `TEMPR_BENCH_SCROLL`), `ResultGrid` scroll handle; code review (8 findings) → `track_scroll` had never been attached (the first bench measured a static viewport), eager run id + pending cancels, `spawn_in`/`update_in` instead of a render hook, headless abort on every terminal path, strict scroll; headless bench numbers discarded (display was off → 1 s frames); user re-ran with display on: **57.2 fps / avg 17.5 / p95 23.1 ms / 61 dropped of 603** — Phase 1 grid box checked with the 10%-slow-frames caveat; 82 unit tests; **PR #6 merged**. Then branch `feat/ph1-service-lifecycle-pool`: `deadpool`-backed `ConnectionService` (user pool + metadata slot, `PooledConnection`, `PoolConfig`, tests for pool growth/reuse/metadata isolation/stop), `Service` impls for the three core services + registry registration, `SchemaService` on the metadata slot, `deadpool-postgres` dropped (→ D19); review (8 findings): `is_closed` on the driver trait + recycle eviction, `stop_all` wired to `on_app_quit`, pool + PG connect timeouts, single `ConnEntry` map with Connecting guard and race-safe insert, bounded concurrent `cancel_all`, `?`-style execute closure, doc names; 90 unit tests; **PR #7 merged**. Then branch `feat/ph1-pg-tls`: `TlsMode` (+2 domain tests), rustls connector module in `tempr_db_postgres` (+2 tests), `?sslmode=` in `DATABASE_URL`, manifest `tls` field, 5 TLS integration tests vs a self-signed `ssl=on` container (→ D20); review (8 findings): SqlState-based error classification with cause chain, libpq `prefer` plaintext fallback, connectors built once and shared, per-run cancel timeout, `TlsChoice` removed, `TlsMode` API trimmed, sharper TLS tests; CLAUDE.md phase pointer flipped, 26 decisions-log rows archived; 94 unit + 15 integration — **Phase 1 checklist complete**; **PR #8 merged**. Then branch `feat/ph2-rope-buffer`: new `tempr_editor` crate, `Buffer` on `ropey` (byte-offset API, atomic validated batch edits, undo/redo transactions with final-position bookkeeping, point↔offset with CRLF/multibyte), 9 tests + 10 MB release probe 1.73 µs avg (→ D21); review (8 findings) → LIFO undo, tie-breaks, no-op edits, LF/CRLF-only lines; 109 unit tests; **PR #9 merged**. Then branch `feat/ph2-syntax-tree`: `SyntaxTree` on `tree-sitter-sequel` (→ D22), lazy incremental reparse from recorded `InputEdit`s, statement ranges / `statement_at`, highlights; probes: edit 5.9 µs, reparse 1.6 ms realistic / 150 ms pathological | command palette + keybindings; editing ops; execute-statement-under-cursor; workspace open + connection picker | TLS connector for PG (last Phase 1 box); workspace open + connection picker; `DriverConnection::ping` health checks; profile the 10% > 20 ms frames |
| 2026-07-14 | Phase 1 | Code review fix pass (10 findings): fixed typed column decoding (finding #1), params passthrough (#2), RETURNING rows (#3), batch-size chunking (#4), schema error propagation (#5), schema scope for columns/indexes (#6), index columns via pg_index (#7), cancel handle capture (#8), configurable sslmode (#9), conninfo escaping via Config builder (#10); reverted #4 from `query_raw` to `query()`+chunked batch due to live-DB `Closed` error with `query_raw` through QueryService (true lazy streaming deferred to TODO); cleaned up debug pollution from root-cause investigation | Lazy wire streaming as follow-up; commit and PR |
