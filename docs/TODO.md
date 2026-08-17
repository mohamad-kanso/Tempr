# Tempr — Backlog

> Ideas and small tasks that surface mid-build but don't belong to the
> current phase. Move items here immediately (living-docs rule in
> CLAUDE.md); promote them into a phase via docs/PROGRESS.md when picked
> up. Now = should land within the current/next phase · Next = upcoming
> phases · Later = pre-release · Ideas = unscoped.

## Now

- [ ] Lazy wire streaming via `query_raw()` — `PostgresStream` currently buffers full result set via `client.query()`, then yields `batch_size` chunks. True `query_raw` streaming was attempted but fails with `Error { kind: Closed }` specifically when called through `QueryService::execute()` (works fine via raw driver or closures). Root cause undetermined — suspected `Box<dyn DriverConnection>` + async trait boundary interaction. Investigate as follow-up.

- [ ] Add `rust-toolchain.toml` pinning `channel = "1.95.0"` (Zed's pin, edition 2024) — prerequisite for the GPUI dependency (→ D16)
- [ ] Verify the license of Zed's `gpui_tokio` crate. Apache-2.0 → depend on it; GPL → reimplement the bridge in Tempr (tokio `Runtime` as a GPUI global + `Task` adapter) (→ D16)
- [ ] Pick and record the `gpui`/`gpui_platform` rev SHA; add both to `[workspace.dependencies]` and confirm `cargo deny` accepts the git sources (incl. transitive `zed-font-kit` on macOS)

## Next

- [ ] Phase 1: PostgreSQL async driver with TLS connection (sslmode configurable, default Prefer)
- [ ] Phase 1: GPUI application shell — main window, text input area, scrollable result grid (base on `crates/gpui/examples/hello_world.rs`; text input from `examples/input.rs`)
- [ ] Phase 1: `gpui_compat` module — wrap `gpui` + `gpui_platform` bootstrap/window/executor calls so upstream churn stays isolated (→ D16)

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
