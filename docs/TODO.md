# Tempr — Backlog

> Ideas and small tasks that surface mid-build but don't belong to the
> current phase. Move items here immediately (living-docs rule in
> CLAUDE.md); promote them into a phase via docs/PROGRESS.md when picked
> up. Now = should land within the current/next phase · Next = upcoming
> phases · Later = pre-release · Ideas = unscoped.

## Now

- [ ] Scaffold Cargo workspace crates per docs/14-project-layout.md (`tempr-core`, `tempr-db`, `tempr-editor`, `tempr-ui`, `tempr-plugin`, `tempr-app`)
- [ ] Implement domain types (`Workspace`, `Connection`, `Query`, `QueryResult`) per docs/03-domain-model.md; unit tests required
- [ ] Implement typed event bus per docs/06-event-system.md; test ordering and delivery guarantees
- [ ] Implement service registry (register, lookup, start/stop lifecycle) per docs/05-services.md; test with mock services
- [ ] Implement workspace file format (read/write round-trip, structured errors for malformed files) per docs/04-workspace.md
- [ ] Implement storage layer (platform-specific data directory R/W) per docs/07-storage.md
- [ ] Set up CI pipeline: `cargo fmt --check`, `cargo clippy`, `cargo test`, `cargo deny` on every push
- [ ] Resolve Open Decision #5 (GPUI fork strategy) before finalizing Cargo.toml — affects whether GPUI is pinned to a commit hash or a fork branch (→ docs/PRODUCT.md OD#5)
- [ ] Resolve Open Decision #2 (license) before publishing any code publicly (→ docs/PRODUCT.md OD#2)

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
