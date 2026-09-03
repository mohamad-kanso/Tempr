# Tempr — Backlog

> Ideas and small tasks that surface mid-build but don't belong to the
> current phase. Move items here immediately (living-docs rule in
> CLAUDE.md); promote them into a phase via docs/PROGRESS.md when picked
> up. Now = should land within the current/next phase · Next = upcoming
> phases · Later = pre-release · Ideas = unscoped.

## Now

- [ ] Lazy wire streaming via `query_raw()` — `PostgresStream` currently buffers full result set via `client.query()`, then yields `batch_size` chunks. True `query_raw` streaming was attempted but fails with `Error { kind: Closed }` specifically when called through `QueryService::execute()` (works fine via raw driver or closures). Root cause undetermined — suspected `Box<dyn DriverConnection>` + async trait boundary interaction. Investigate as follow-up.

- [ ] Make `ConnectionService` / `QueryService` / `SchemaService` implement the `Service` lifecycle trait so the binary registers them in `ServiceRegistry` (today only test services implement it; the GPUI shell holds plain `Arc`s)
- [ ] Wire `deadpool-postgres` pool into `ConnectionService` (dependency declared, unused)
- [ ] `Input` component: hand-roll single-line text input from `crates/gpui/examples/input.rs` (rev `ed8d600`) — Phase 1 editor placeholder
- [ ] `ThemeProvider` tokens replace the placeholder palette consts in `tempr_ui::main_window` (no hard-coded colors rule, 11-gpui.md)
- [ ] `Table` component: `uniform_list`-backed result grid fed by `QueryService` stream via `gpui_compat::spawn_tokio` — Phase 1 grid placeholder

## Next

- [ ] Phase 1: PostgreSQL async driver with TLS connection (sslmode configurable, default Prefer)
- [ ] Phase 1: main window layout beyond placeholders — connection picker, status bar, error toasts driven by `AppEvent`
- [ ] macOS/Windows CI runners for the gpui build (Linux-only today; transitive `zed-font-kit` git source on macOS must pass `cargo deny`)

## Later

- [ ] Phase 2: Rope buffer implementation (10 MB, sub-ms insert/delete)
- [ ] Phase 2: Tree-sitter PostgreSQL grammar integration + incremental parse
- [ ] Phase 2: Statement boundary detector ($$ delimiters, comments, string literals)
- [ ] Phase 2: Command palette (fuzzy search, all registered commands, configurable keybindings)
- [ ] Phase 2: Keyboard-only audit — every user-facing action listed with keybinding
- [ ] Phase 3: Catalog cache (schema metadata from PostgreSQL, local cache, incremental refresh)
- [ ] Phase 3: Completion provider (context-aware, < 5 ms, 10,000 objects)
- [ ] Phase 3: Semantic analyzer (column ref resolution, ambiguity detection)
- [ ] Phase 3: Real-time diagnostics (syntax via tree-sitter + semantic via analyzer)
- [ ] Phase 3: Hover type information
- [ ] Phase 4: Plugin API stabilization (versioned, documented)
- [ ] Phase 4: Theme system (light/dark, configurable accent)
- [ ] Phase 4: Query history panel
- [ ] Phase 4: Layout persistence across restarts
- [ ] Phase 4: Platform-native packaging (.deb, .rpm, AppImage, .dmg, MSI)
- [ ] Resolve Open Decisions #1 (name/branding), #3 (telemetry), #4 (beta timing) before Phase 4

## Ideas

- [ ] WASM-based plugin execution sandbox (post-v1; requires ABI layer on top of stable plugin API)
- [ ] AI-assisted query generation via LLM provider abstraction (post-v1; Phase 3 catalog cache provides schema context)
- [ ] Real-time collaboration / multi-cursor shared SQL editing (post-v1; different architecture concern)
- [ ] MySQL/SQLite driver (post-v1; driver abstraction from Phase 1 supports this — just implement `DatabaseDriver`)
- [ ] Theme marketplace / plugin registry (post-v1; depends on Phase 4 plugin system stability)
- [ ] Cross-platform dark/light mode sync with OS appearance setting
- [ ] Query explain/analyze visualization panel
