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
- [ ] `ThemeProvider` tokens replace the placeholder palette consts in `tempr_ui::theme` (no hard-coded colors rule, 11-gpui.md)
- [ ] Scroll bench shows ~1% frames > 20 ms (max 83–100 ms) at 100k rows in release — profile (first-frame layout? text shaping of the incoming row window? allocator?) and add a per-frame histogram to `ScrollBench`
- [ ] `execute_streaming` should hand back the `QueryRunId` before completion (e.g. return `(QueryRunId, impl Future)` or take a pre-allocated id) so `CancelQuery` targets one run instead of `active_runs()`
- [ ] Connection picker in `MainWindow` (workspace connection list) replaces the `DATABASE_URL` stand-in; surface a malformed `DATABASE_URL` in the UI instead of exiting before the window opens
- [ ] `ResultGrid`: column virtualization + resize, copy-on-select, 2D keyboard navigation (11-gpui.md Table row); columnar `RowStore` with spill-to-disk (13-result-grid.md) once result sizes demand it
- [ ] `Input`: undo/redo, auto-resize; multi-line SQL input arrives with the Phase 2 editor (rope + tree-sitter)

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
