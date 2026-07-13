# Tempr — Major Decisions Record

> **The WHY ledger, across sessions.** Every MAJOR decision — made by the
> user or by Claude in any session — gets a numbered entry here in the
> same turn it is made, so no future session re-litigates a settled
> question. Fine-grained session choices stay in docs/PROGRESS.md's
> decisions log; when a log row records a major decision it links here
> (`→ D<n>`) instead of restating the rationale.
>
> **Major means any of**: changes the architecture, stack or a pattern
> rule (incl. new dependencies) · changes business rules or user-facing
> behavior · deviates from the blueprint or the design export · closes a
> PRODUCT.md open decision · an explicit user directive.
> Before overturning an entry, read it — then supersede it with a new
> entry carrying `Supersedes: D<n>` (never edit history). The ONE allowed
> edit to an old entry: add a `**Superseded by**: D<m>` line under its
> title, so a reader landing on it is redirected forward.

## Index

| # | Date | Decision | By |
|---|------|----------|----|
| D1 | 2026-07-13 | GPUI as sole UI framework — no egui, Qt, Electron, Tauri, iced | User (setup) |
| D2 | 2026-07-13 | Rust-only for app and v1 plugins — no FFI, no other languages | User (setup) |
| D3 | 2026-07-13 | Custom SQL editor — never embed Monaco, CodeMirror, or Scintilla | User (setup) |
| D4 | 2026-07-13 | PostgreSQL first via driver abstraction (other engines post-v1) | User (setup) |
| D5 | 2026-07-13 | Workspace-first scope model — everything scoped to a workspace directory | User (setup) |
| D6 | 2026-07-13 | Service-oriented architecture — no business logic in UI components | User (setup) |
| D7 | 2026-07-13 | Internal event bus for inter-service communication | User (setup) |
| D8 | 2026-07-13 | Plugin-extensible everything from day one; core features are static plugins | User (setup) |
| D9 | 2026-07-13 | Internal semantic engine, not external LSP — in-process, no process-boundary latency | User (setup) |
| D10 | 2026-07-13 | Capability-gated roadmap phases over date-based milestones | User (setup) |
| D11 | 2026-07-13 | Lorekeeper living-docs system adopted; PRODUCT/PROGRESS/TODO/DECISIONS bootstrapped | User + Claude |

---

## D1 — GPUI as sole UI framework (2026-07-13)

**By**: User (setup).
**Decision**: Use GPUI (from the Zed project) as the sole UI framework. No egui, no Qt, no Electron/Tauri, no iced.
**Why**: GPUI delivers GPU-accelerated retained-mode rendering, deep theming, and virtualized list performance at a level the alternatives cannot match while staying in pure Rust. egui lacks theming depth; Qt introduces C++ FFI; Electron violates the performance pillar; iced's virtualized list widgets were insufficiently mature at evaluation time. Full rationale in docs/adr/0001-gpui-for-ui.md.
**Consequences**: GPUI is a bleeding-edge dependency tied to Zed's development cadence — API changes can be breaking. A custom component layer must wrap all GPUI usage to absorb churn. Mitigate by pinning to a specific revision (resolve Open Decision #5 before Phase 0).

---

## D2 — Rust-only constraint (2026-07-13)

**By**: User (setup).
**Decision**: Tempr is written entirely in Rust. No other languages for the application or v1 plugins. No FFI boundaries in UI code.
**Why**: Single toolchain, no GC pauses, no FFI unsoundness risk, direct GPU access. A mixed-language project would complicate GPUI integration and introduce runtime overhead inconsistent with the performance pillar. Full rationale in docs/adr/0002-rust-only.md.
**Consequences**: Plugin authors must write Rust crates at v1. WASM plugins (post-v1) would relax this for plugin authors without changing the core. No JavaScript, Python, or C bindings in v1.

---

## D3 — Custom SQL editor, no embedded editors (2026-07-13)

**By**: User (setup).
**Decision**: Build a custom SQL editor (rope buffer + tree-sitter + custom rendering). Never embed Monaco, CodeMirror, Scintilla, or any web-based or C++ editor component.
**Why**: Embedded editors introduce web views (Monaco/CodeMirror) or C++ FFI (Scintilla), violating the native/Rust-only pillars. A custom editor built on GPUI is the only path to seamless integration with the semantic engine, command palette, and keyboard-first UX. Full rationale in docs/adr/0003-custom-sql-editor.md.
**Consequences**: Significant Phase 2 investment. The rope buffer, tree-sitter integration, and statement detector must be built from scratch.

---

## D4 — PostgreSQL first via driver abstraction (2026-07-13)

**By**: User (setup).
**Decision**: Support PostgreSQL as the first and only database at v1. All driver code is written behind a `DatabaseDriver` trait abstraction so additional engines (MySQL, SQLite) can be added post-v1 without restructuring the execution pipeline.
**Why**: Focusing on one engine allows deep, high-quality integration rather than shallow multi-engine support. The trait abstraction ensures the architecture stays clean for future engines. Full rationale in docs/adr/0004-postgresql-first.md.
**Consequences**: v1 is PostgreSQL-only. The driver trait must be designed carefully in Phase 1 to avoid PostgreSQL-isms leaking into the interface.

---

## D5 — Workspace-first scope model (2026-07-13)

**By**: User (setup).
**Decision**: Everything in Tempr is scoped to a workspace directory. A workspace contains connection configs, query files, settings, and history. The workspace directory is the unit of collaboration and persistence.
**Why**: Workspace-first enables reproducible, version-controllable database work environments. It is the natural model for developers who already work directory-first in their editors. Full rationale in docs/adr/0005-workspace-first.md.
**Consequences**: No global state that outlives a workspace (except application-level preferences). Opening the app without a workspace shows a workspace picker, not a blank slate.

---

## D6 — Service-oriented architecture (2026-07-13)

**By**: User (setup).
**Decision**: No business logic in UI components. Views call services; services contain logic; services publish events; views subscribe to events. This separation is absolute — it is a product invariant.
**Why**: Service-oriented architecture enables independent testing of business logic, clean service substitution (e.g., mock drivers in tests), and clear ownership boundaries. It also enables the plugin system to extend services without touching views. Full rationale in docs/adr/0006-service-oriented-architecture.md.
**Consequences**: UI components are thin. A view that needs data calls a service and awaits an event, never computes the answer itself. More files and indirection in exchange for testability and extensibility.

---

## D7 — Internal event bus for inter-service communication (2026-07-13)

**By**: User (setup).
**Decision**: Services communicate via a typed internal event bus. Services publish events; other services and views subscribe. Direct service-to-service calls are permitted only for synchronous, non-I/O operations.
**Why**: An event bus decouples producers from consumers, enabling plugin services to subscribe to core events without the core knowing about plugins. It also provides a natural boundary for async fan-out. Full rationale in docs/adr/0007-internal-event-bus.md.
**Consequences**: Event types must be kept stable as the plugin API matures. The event taxonomy (docs/06-event-system.md) is a contract surface.

---

## D8 — Plugin-extensible everything from day one (2026-07-13)

**By**: User (setup).
**Decision**: The plugin system is built alongside core features, not after. Core features (result formatters, export commands, completion providers) are implemented as static plugins using the same public API, ensuring the API is battle-tested. Plugins register capabilities with the host; they never receive direct access to the service registry.
**Why**: A plugin API designed after the fact reflects convenience rather than real capability. By dogfooding the plugin API with core features, v1 ships with a verified, battle-hardened extension surface. Full rationale in docs/adr/0008-plugin-system.md.
**Consequences**: Phase 0 must define the plugin registration interface even before any core feature is implemented. Phase 4 is about stabilizing and documenting an API that has been in use since Phase 1, not inventing a new one.

---

## D9 — Internal semantic engine, not external LSP (2026-07-13)

**By**: User (setup).
**Decision**: Build an internal SQL semantic engine that runs in-process. Use LSP concepts (requests, capabilities) as internal API shapes for familiarity, but do not use the LSP protocol or spawn a separate language server process.
**Why**: The process boundary of an external LSP adds latency on every keystroke, blocks direct access to the workspace's catalog cache, and limits features to what the protocol supports. The < 5 ms completion budget is unachievable with a protocol hop. The SQL LSP ecosystem (sqls, postgres_lsp) is immature compared to what Tempr needs. Full rationale in docs/adr/0009-internal-semantic-engine-not-lsp.md.
**Consequences**: No ecosystem reuse from the SQL LSP world. The semantic engine (docs/12-sql-intelligence.md) must be built from scratch, including the catalog cache, completion ranker, and semantic analyzer. This is Phase 3 scope.

---

## D10 — Capability-gated roadmap over date-based milestones (2026-07-13)

**By**: User (setup).
**Decision**: Phases are completed when exit criteria are verifiably met, not when a calendar date arrives. No phase N+1 work begins until phase N's exit criteria pass. Full rationale and phase definitions live in docs/16-roadmap.md.
**Why**: Date-based milestones produce scope cuts and ship bugs. Native infrastructure (GPUI, rope buffers, tree-sitter, async driver stack) has real subsystem dependencies that a Gantt chart cannot predict. Capability gates produce reliable results at the cost of schedule uncertainty. Full rationale in docs/16-roadmap.md §Design Rationale.
**Consequences**: Phases can pause on hard blockers without pretending they don't exist. Quarterly reviews adjust future phases without retroactively relaxing exit criteria.

---

## D11 — Lorekeeper living-docs system adopted (2026-07-13)

**By**: User + Claude.
**Decision**: The lorekeeper living-docs system is adopted for this project. PRODUCT.md, PROGRESS.md, TODO.md, and DECISIONS.md are the four living docs; CLAUDE.md carries the operating contract and update rules. All four are updated in the same turn as the work they reflect.
**Why**: The project was in architecture/documentation phase with rich docs (16 architecture docs, 9 ADRs) but no session-to-session continuity system. Lorekeeper provides cold-start capability, prevents re-litigating settled decisions, and keeps docs honest against the code.
**Consequences**: Every session starts with the PROGRESS status block. Every major decision gets a DECISIONS.md entry. New ideas go to TODO immediately, never left in conversation.
