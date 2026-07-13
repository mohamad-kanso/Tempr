# Tempr — Backlog

> Ideas and small tasks that surface mid-build but don't belong to the
> current phase. Move items here immediately (living-docs rule in
> CLAUDE.md); promote them into a phase via docs/PROGRESS.md when picked
> up. Now = should land within the current/next phase · Next = upcoming
> phases · Later = pre-release · Ideas = unscoped.

## Now

- [ ] Resolve Open Decision #5 (GPUI fork strategy) before adding GPUI as a dependency — affects whether it is pinned to a commit hash or a fork branch (→ docs/PRODUCT.md OD#5)

## Next

- [ ] Phase 1: PostgreSQL async driver (`sqlx` or custom) with TLS connection, query execution, streaming result batches
- [ ] Phase 1: GPUI application shell — main window, text input area, scrollable result grid
- [ ] Phase 1: Streaming result grid with bounded memory (virtualized rendering)
- [ ] Phase 1: Integration tests for connect / auth failure / query / streaming paths

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
