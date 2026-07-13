# Tempr — Progress

> **Single source of truth for where the build stands.** Update after every
> completed task per the living-docs rule in CLAUDE.md. Phase definitions
> live in docs/16-roadmap.md; business scope in docs/PRODUCT.md.

## Current status

- **Last completed**: Living-docs bootstrapped (lorekeeper adopt, 2026-07-13); architecture documentation suite complete (16 docs + 9 ADRs + RFC process, commit `6df466a`) → D11
- **Verified**: Docs exist and cross-reference correctly (manual check, 2026-07-13). Code check commands (cargo fmt, clippy, test) not yet run — stub `main.rs` only, no implementation.
- **Next action**: Begin Phase 0 — create Cargo workspace structure per docs/14-project-layout.md, implement domain types.

## Phase checklist

*Reconstructed from git history on 2026-07-13. Two commits total: initial project scaffold + full documentation suite. No implementation code yet.*

### Phase 0 — Foundations
- [ ] Cargo workspace builds with zero warnings on Linux, macOS, Windows
- [ ] Domain types (`Workspace`, `Connection`, `Query`, `Result`) compile and pass unit tests
- [ ] Event bus dispatches typed events to registered handlers; ordering and delivery verified by tests
- [ ] Service registry supports registration, lookup, and lifecycle (start/stop) with mock services
- [ ] Workspace file format is read/write round-trip safe; malformed files produce structured errors
- [ ] Storage layer writes and reads structured data to platform-specific data directory
- [ ] CI pipeline runs `cargo fmt --check`, `cargo clippy`, `cargo test`, `cargo deny` on every push
- [x] All 16 architecture docs exist and cross-reference correctly *(verified 2026-07-13)*

### Phase 1 — Connect & Run
- [ ] PostgreSQL driver connects over TLS using Phase 0 connection config
- [ ] `SELECT` and `INSERT` execute and return results through the event bus
- [ ] Streaming result pipeline delivers rows in bounded memory
- [ ] GPUI window renders with text input and scrollable result grid
- [ ] Result grid displays streaming rows; smooth scroll for up to 100,000 rows
- [ ] Connection/auth/syntax errors produce user-visible messages via event system
- [ ] Integration tests cover connect, auth failure, query execution, streaming for PostgreSQL

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

## Session log

| Date | Phase | What was done | Follow-ups |
|---|---|---|---|
| 2026-07-13 | docs | Architecture suite (16 docs + 9 ADRs + RFC process) written; lorekeeper living-docs bootstrapped (PRODUCT, PROGRESS, TODO, DECISIONS, CLAUDE.md updated) | Begin Phase 0: Cargo workspace + domain types |
