# Tempr — Product Source of Truth

> Business & requirements document. Derived from CLAUDE.md,
> docs/16-roadmap.md, and the architecture docs in docs/. **Keep in sync
> per the living-docs rule in CLAUDE.md.**

## What the app is

**Tempr is a native Rust database IDE** that pairs **Zed's speed and keyboard-first philosophy** with **DataGrip's deep SQL intelligence** — for developers who write SQL daily, not DBA administrators who manage infrastructure. It is not a DBeaver clone: it optimizes for the developer workflow (keyboard-first, command-palette-driven, instant startup) rather than universal database administration. PostgreSQL is the first and only supported database at v1.

## Users & roles

| Role | Demo persona | What they do | Surface |
|---|---|---|---|
| SQL Developer | Backend engineer, data engineer | Writes SQL daily, executes queries, inspects schemas, exports results | Editor, result grid, schema browser, command palette |
| Power User | Data scientist, analyst | Complex multi-statement queries, large result sets, query history | All surfaces + query history panel |

## Features & acceptance criteria

Status: ✅ implemented, 🔜 planned phase, per docs/PROGRESS.md.

### 1. Foundations (Phase 0) ✅
- ✅ Cargo workspace builds with zero warnings on Linux, macOS, Windows
- ✅ Domain types (`Workspace`, `Connection`, `Query`, `Result`) compile and pass unit tests
- ✅ Event bus dispatches typed events to registered handlers with verified ordering guarantees
- ✅ Service registry supports registration, lookup, and lifecycle (start/stop)
- ✅ Workspace file format is read/write round-trip safe; malformed files produce structured errors
- ✅ Storage layer writes and reads structured data to the platform-specific data directory
- ✅ CI pipeline runs `cargo fmt --check`, `cargo clippy`, `cargo test`, `cargo deny` on every push
- ✅ All 16 architecture docs exist and cross-reference correctly
- AC: `cargo test` exits 0. Malformed workspace manifest produces a typed error, not a panic. Event bus delivers events to all registered handlers in registration order.

### 2. Connect & Run (Phase 1)
- ✅ PostgreSQL driver connects to a live instance using connection config from Phase 0 (TLS configurable via libpq `sslmode` values, default `prefer`; rustls with platform roots — verified against a TLS-enabled PostgreSQL 16 on 2026-09-03)
- ✅ `SELECT` and `INSERT` execute and return rows or affected-row counts through the service layer
- 🔜 Streaming result pipeline delivers rows in batches (`batch_size`-sized); memory usage bounded regardless of result size *(batches + `RowSink` ✅; bounded memory ✗ — the PostgreSQL driver still buffers via `client.query()` and the grid keeps all rows; see TODO "Lazy wire streaming" and `RowStore`)*
- ✅ GPUI application window renders with a text input area and a scrollable result grid
- ✅ Result grid displays streaming rows as they arrive; scrolling is smooth for result sets up to 100,000 rows *(release, display on: 57.2 fps, p95 23.1 ms, 10% of frames > 20 ms under the worst-case full-viewport-jump benchmark; see TODO)*
- ✅ Connection errors, auth failures, and query syntax errors produce user-visible messages (status bar)
- AC: 100,000-row result set scrolls at 60 fps. Auth failure produces a user-visible error (not a crash). Peak RSS stays below 500 MB streaming 1,000,000 rows.

### 3. Editor (Phase 2)
- ✅ Rope buffer handles documents up to 10 MB with sub-millisecond insert/delete at arbitrary positions (`tempr_editor::Buffer` on `ropey`; measured 1.73 µs avg / 12.1 µs worst in release, 2026-09-03)
- ✅ Tree-sitter PostgreSQL grammar produces an incremental syntax tree (`tree-sitter-sequel`; lazy incremental reparse: 1.6 ms on 10 MB of realistic statements, 150 ms worst case for a 180k-statement dump; 2026-09-03)
- ✅ Statement detector correctly identifies statement boundaries (respecting `$$` delimiters, comments, string literals) — derived from the tree-sitter tree (2026-09-03)
- 🔜 Command palette opens via keybinding, lists all registered commands, accepts fuzzy input, executes selected command *(implemented 2026-09-05; interactive confirmation pending)*
- 🔜 Keybindings are configurable via the workspace format; a default keybinding map is provided *(default map + user-level `~/.config/tempr/settings.toml` `[keybindings]` live 2026-09-05; the `workspace.toml` layer is parsed but applied only once workspace open lands)*
- ✅ Cursor movement, selection, copy/paste, undo/redo, and line operations work on the rope buffer (model layer: `tempr_editor` motions + multi-cursor edit ops, selection-restoring undo/redo; the `EditorView` UI drives them through 33 keyboard commands; 2026-09-05)
- 🔜 "Execute statement under cursor" works end-to-end *(implemented 2026-09-05 via `EditorView` → `Buffer::statement_at` → `QueryService`; interactive confirmation pending)*
- 🔜 No mouse action required for any editor operation
- AC: Every user-facing action appears in a published keybinding audit table. Insert/delete at mid-document position in a 10 MB file completes in < 1 ms.

### 4. SQL Intelligence (Phase 3)
- 🔜 Catalog cache loads schema metadata (databases, schemas, tables, columns, types, indexes) from PostgreSQL
- 🔜 Completion provider offers context-aware suggestions ranked by relevance
- 🔜 Completion latency from keystroke to popup < 5 ms for cached catalogs with up to 10,000 schema objects
- 🔜 Semantic analyzer resolves column references; detects ambiguous and unresolvable names
- 🔜 Diagnostics produced for syntax and semantic errors in real time
- 🔜 Hover information displays column types and table definitions
- 🔜 All intelligence features work offline from local cache after initial load; no I/O on the request path
- AC: Completion popup appears in < 5 ms on a cold (no revalidation) catalog with 10,000 objects. Typing an unknown column reference produces a diagnostic underline.

### 5. Extensibility & Polish (Phase 4)
- 🔜 Plugin API declared stable: versioned, documented, backward-compatible within major versions
- 🔜 Core features migratable to plugin API as reference implementations
- 🔜 Theme system supports light and dark themes with configurable accent colors
- 🔜 Query history persisted and browsable via a dedicated UI panel
- 🔜 Layout state (panel sizes, open files, cursor positions) persists across restarts
- 🔜 A third-party-style plugin adds a working panel and completion provider using only the public API
- 🔜 Platform-native bundles for Linux (.deb, .rpm, AppImage), macOS (.dmg), Windows (MSI)
- AC: A plugin developed outside the core crates installs, activates its panel, and provides completions without modifying any core file.

## Platform & constraints

- Targets: Linux (glibc 2.31+), macOS (12+), Windows (10+)
- Single-user, local-only; no cloud sync, no telemetry without explicit opt-in
- GPUI requires GPU with Vulkan/Metal/DX12 support
- GPUI is consumed as `gpui` + `gpui_platform` (Apache-2.0) pinned to one Zed rev. Tempr is MIT, so Zed's GPL-3.0 crates (`ui`, `theme`, `markdown`, `editor`) are out of bounds — every UI component, the theme system, and the text input are Tempr-authored (→ D16); the pinned rev must be ≥ Zed `ac5af8b9` and `cargo deny check licenses` gates every bump (→ D17)
- Toolchain floor: Rust 1.95.0, edition 2024 (matches Zed's pin)
- All I/O is async; UI thread is render-only
- Workspace files are local directories; no remote workspace in v1

## Non-functional requirements

- Startup to first interactive frame: < 200 ms (cold start, no workspace loaded, mid-range machine)
- Completion latency: < 5 ms keystroke → popup (10,000 cached schema objects)
- Result scroll: 1,000,000 rows at 60 fps, peak RSS < 500 MB
- Every user-facing action must have a keyboard equivalent (enforced by keybinding audit at v1)
- No synchronous I/O before first frame except the workspace manifest

## Out of scope (V1)

- DBA administration (instance management, user management, role grants)
- Additional database engines — MySQL, SQLite, etc. (driver abstraction is designed for them; adding one means implementing `DatabaseDriver`, not restructuring; post-v1)
- WASM plugin execution (native Rust plugins only at v1)
- AI-assisted query generation (post-v1; intelligence layer from Phase 3 would provide schema context)
- Collaboration / real-time multi-user editing
- Theme marketplace / plugin registry (infrastructure deferred post-v1)
- Remote workspaces

## Open Decisions

| # | Question | Options / leaning | Blocking? |
|---|---|---|---|
| 1 | **Name & branding** — "Tempr" is a working name; logo, domain needed | Keep "Tempr" vs. rename; no strong leaning yet | Blocks any public release or repo exposure |
| 3 | **Telemetry policy** — even anonymized telemetry requires explicit opt-in | Opt-in only (product invariant already set); policy text and toggle UX need design | Blocks Phase 4 / any beta |
| 4 | **Beta program timing** — after Phase 2 (usable editor) or Phase 3 (intelligence)? | Earlier = more feedback but rougher experience | Affects Phase 3/4 sequencing |
| 6 | **Plugin distribution** — central registry vs. Git-based vs. both? | Git-based is simpler first; registry adds discovery | Blocks Phase 4 plugin story |
| 7 | **Release cadence post-v1** — fixed schedule (e.g. 8-week) vs. rolling? | Rolling is simpler; fixed gives users predictability | Post-v1 only |
| 8 | **Multi-engine catalog caching** — per-engine namespace or unified model? | Per-engine is simpler; unified is more powerful | Affects Phase 3 architecture (must decide before Intelligence phase) |
