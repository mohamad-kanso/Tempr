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
| D12 | 2026-07-13 | MIT license chosen as working default — closes OD#2 | Claude (Phase 0) |
| D13 | 2026-07-13 | PR review-based development workflow — branch → PR → /code-review → user approval → merge | User |
| D14 | 2026-07-13 | GPUI dependency via upstream git pin (no fork) — `rev = "<sha>"`, updated deliberately | User |
| D15 | 2026-07-13 | No direct commits to main ever — all work via branch → PR → review → merge, no exceptions | User |
| D16 | 2026-08-17 | GPUI dependency is `gpui` + `gpui_platform` at one rev; Apache-2.0 Zed crates only, no GPL `ui`/`theme`/`markdown`/`editor` | Claude (Phase 1) |
| D17 | 2026-09-03 | GPUI pinned to Zed `main` `ed8d600` with floor `ac5af8b9` (zlog/ztracing relicense); `gpui_tokio` adopted; `cargo deny check licenses` is the enforcement gate; toolchain `1.97.1` | Claude (Phase 1) |
| D18 | 2026-09-03 | Small pure-Rust utility crates are adopted without an RFC when already in the graph: `unicode-segmentation` (grapheme cursor motion), `percent-encoding` (URL userinfo decoding), `url` promoted to a runtime dep | Claude (Phase 1) |
| D19 | 2026-09-03 | Connection pooling = `deadpool` (core, `managed`) over `Box<dyn DriverConnection>` in `ConnectionService`; user pool (max 8) + dedicated 1-slot metadata pool; `deadpool-postgres` dropped | Claude (Phase 1) |
| D20 | 2026-09-03 | PostgreSQL TLS via rustls (`tokio-postgres-rustls`, `ring` provider, platform roots from `rustls-native-certs`); `TlsMode` on `Connection` with libpq `sslmode` semantics, default `prefer`; `verify-ca` treated as `verify-full` | Claude (Phase 1) |
| D21 | 2026-09-03 | Editor buffer = `ropey` 1.x rope in new `tempr_editor` crate; public API is byte-offset based (ropey char indices never leak); `edit` is a validated, atomic batch returning `Result`; `Buffer` is a pure model (no event bus) | Claude (Phase 2) |
| D22 | 2026-09-03 | SQL grammar = `tree-sitter-sequel` (DerekStride, MIT) on `tree-sitter` 0.25; `Buffer` records `InputEdit`s eagerly but re-parses **lazily** on read, so `edit` stays sub-ms on any document size | Claude (Phase 2) |
| D23 | 2026-09-05 | Commands = GPUI action types in one typed catalog (`tempr_ui::commands`); `CommandService` owns metadata + layered keybindings (defaults ← `~/.config/tempr/settings.toml` ← `workspace.toml` `[keybindings]`, command → keystrokes, empty = unbind); palette = `Input` + `uniform_list` over in-house fuzzy search; execution stays in the UI, service records `CommandExecuted` | Claude (Phase 2) |
| D24 | 2026-09-07 | Workspace keybinding layer is applied at startup from a `workspace.toml` discovered by path (`TEMPR_WORKSPACE`, else the current directory) — ahead of workspace open; sync `parse_manifest`/`load_manifest_from` sit beside the async `Storage` trait for pre-runtime callers | Claude (Phase 2 exit) |
| D25 | 2026-09-08 | Schema object identity is derived (UUIDv5 over connection id + kind + native id), not random | Claude (Phase 3 stage 2) |
| D26 | 2026-09-08 | Incremental refresh diffs an `(oid, xmin)` sweep against cached fingerprints, re-introspecting per touched schema | Claude (Phase 3 stage 2) |
| D27 | 2026-09-08 | Catalog cache format (`.tcat`) is bincode 2.x behind a versioned header (magic, version, flags, content hash); any mismatch discards and re-introspects rather than erroring | Claude (Phase 3 stage 2) |

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

---

## D12 — MIT license chosen as working default (2026-07-13)

**By**: Claude (Phase 0 CI requirement).
**Decision**: `license = "MIT"` set in `[workspace.package]` and inherited by all crates via `license.workspace = true`. OD#2 is closed.
**Why**: `cargo deny check` requires a license field on all workspace crates. MIT is permissive, compatible with all current dependencies, does not restrict plugin authors, and can be superseded before any public release if a copyleft strategy is preferred. Choosing MIT now unblocks CI without foreclosing the AGPL option — a superseding ADR can change it before the repo goes public.
**Consequences**: MIT is the legal default until explicitly superseded. Any license change before public release requires a new DECISIONS.md entry (D13+) and updating `Cargo.toml`. If the project goes AGPL, the entire commit history will carry the MIT header for early commits — inform legal if this matters.

---

## D13 — PR review-based development workflow (2026-07-13)

**Superseded by**: D15.

**By**: User.
**Decision**: All feature-level work (Phase checklist items) follows: `feature-dev` skill → `git checkout -b feat/ph<N>-<slug>` → implement → `/code-review` → `gh pr create` → user approves → `gh pr merge`. No direct pushes to `main`. Enforced locally by `.github/hooks/pre-push` (installed via `scripts/setup.sh`). Branch prefixes: `feat/ph<N>-*`, `fix/*`, `docs/*`, `chore/*`.
**Why**: Agents working directly on `main` bypass review. A lightweight push-gate plus a CLAUDE.md hard rule ensures both human and agent work goes through review before landing. Approach A (CLAUDE.md rule + local git hook) chosen over heavier Claude Code PreToolUse hooks for simplicity.
**Consequences**: Every feature branch requires a PR and a `/code-review` pass before merge. Trivial one-line doc corrections in the same session may still land directly — use judgment. GitHub server-side branch protection is optional at this stage.

---

## D14 — GPUI dependency via upstream git pin (2026-07-13)

**By**: User.
**Decision**: Depend on GPUI from the upstream `zed-industries/zed` monorepo pinned to a specific commit SHA (`rev = "<sha>"`). No fork. Rev is updated deliberately when new GPUI APIs are needed. Closes OD#5.
**Why**: A fork doesn't eliminate the need to track upstream — GPUI is in active development and improvements are needed — it just adds a rebase burden on top. The custom component layer committed in D1 already provides the right abstraction to absorb API churn without forking. Deliberate rev bumps give controlled upgrade cadence without fork maintenance overhead.
**Consequences**: Phase 1 adds `gpui = { git = "https://github.com/zed-industries/zed", rev = "<sha>" }` to the workspace Cargo.toml. The pinned SHA is updated intentionally, not on every Zed release. No fork to maintain. If a patch is ever needed that upstream won't accept, reconsider with a superseding entry.

---

## D15 — No direct commits to main, ever (2026-07-13)

**By**: User.
**Supersedes**: D13.
**Decision**: All work — feature, fix, docs, chore — must go through a branch → PR → `/code-review` → user approval → merge workflow. No direct commits to main under any circumstances. The D13 "trivial doc correction" judgment exception is eliminated.
**Why**: Claude committed docs changes directly to main (OD#5 resolution), citing the judgment exception in D13. That exception is too wide and defeats the point of branch protection. A bright-line rule with no exceptions removes the rationalization surface.
**Consequences**: Even single-line doc fixes go on a branch and through a PR. More process overhead for trivial changes; the tradeoff is an unambiguous rule that cannot be argued around.

---

## D16 — GPUI dependency surface and Zed crate licensing (2026-08-17)

**By**: Claude (Phase 1, from a verified read of a local `zed-industries/zed` clone — gpui `0.2.2`).
**Refines**: D14 (git pin, no fork — unchanged).
**Decision**:
1. Tempr depends on **two** Zed crates, pinned to the **same** rev: `gpui` (framework) and `gpui_platform` (platform entry point). Upstream split the crate; the app bootstraps via `gpui_platform::application()`, never `gpui::Application::new()`.
2. Tempr uses **Apache-2.0 Zed crates only** — `gpui`, `gpui_platform`, and the `gpui_macos`/`gpui_linux`/`gpui_windows`/`gpui_web`/`gpui_wgpu` backends. `crates/ui` (component library), `crates/theme`, `crates/markdown`, and `crates/editor` are **GPL-3.0** and must never be depended on.
3. GPUI's own executor (`cx.background_spawn` / `cx.spawn` / `Task<R>`) is the UI-side async model. Tokio DB work reaches the UI through a tokio↔GPUI bridge (`gpui_tokio` if its license permits, otherwise a Tempr reimplementation).
4. `gpui-component` (longbridge) is not adopted — it conflicts with the custom-component mandate in D1. Adopting it would need an RFC.

**Why**: Tempr is MIT (D12). Linking GPL-3.0 crates would force Tempr to become GPL, which contradicts D12 and the plugin-author-friendly stance in D8. The split into `gpui` + `gpui_platform` is a hard build fact — a single-crate dependency will not compile a window open. Zed's async model is its own scheduler, not tokio, so the DB layer (`tokio-postgres`, `deadpool-postgres`) cannot simply be awaited inside a view; the bridge is mandatory and is better documented before Phase 1 UI work than discovered mid-implementation.

**Consequences**: Every component in the [11 — GPUI](11-gpui.md) catalog is Tempr-authored — there is no shortcut via `crates/ui`, and theming must be built from scratch (no `cx.theme()`). Text input is hand-rolled from `crates/gpui/examples/input.rs` as the reference. `cargo deny` must allow Apache-2.0 git sources including the transitive `zed-font-kit` fork on macOS. Zed pins toolchain `1.95.0`/`edition 2024`; Tempr must add a matching `rust-toolchain.toml` before the GPUI dependency lands. Any future need for a GPL Zed crate requires an RFC plus a superseding entry (and likely a license change to Tempr itself).

---

## D17 — GPUI pin rev, license gate, and tokio bridge (2026-09-03)

**By**: Claude (Phase 1, from `cargo deny check licenses` against the fetched graph and the upstream commit history).
**Refines**: D14 (git pin, no fork), D16 (Apache-2.0 crates only).
**Decision**:
1. `gpui`, `gpui_platform` and `gpui_tokio` are pinned to Zed `main` rev `ed8d6004648e4e38a6879d9c80ac4406ad3d7266` (2026-09-03). Any future pin must be **≥ `ac5af8b9e1ea3f7922fbabefe409c05b8766135c`** (2026-09-01, "Relicense zlog, ztracing, and ztracing_macro under Apache-2.0"). No Zed release tag up to and including v1.18.0 satisfies this.
2. `cargo deny check licenses` is the mechanical gate for D16: the `[licenses].allow` list is permissive-only (MIT, Apache-2.0, BSD, ISC, MPL-2.0, Unicode-3.0, Zlib, CC0-1.0, bzip2-1.0.6, NCSA). No `GPL-*` is ever added. `[sources]` allows only the `zed-industries` GitHub org for git dependencies.
3. `gpui_tokio` (Apache-2.0, verified at the pinned rev) is the tokio↔GPUI bridge. Application code reaches it only through `tempr_ui::gpui_compat::spawn_tokio`.
4. `rust-toolchain.toml` pins `1.97.1` (Zed `main`'s pin at this rev) with the minimal profile plus `rustfmt`/`clippy`. It is bumped together with the gpui rev.

**Why**: At tag v1.9.0 the gpui dependency tree contained three first-party Zed crates declared `GPL-3.0-or-later` (`zlog` ← `ztracing` ← `sum_tree` ← `gpui`). Linking them would have made Tempr GPL, contradicting D12/D16, and the release tags — the "reviewed, stable" pin points D14 prefers — all predate the fix. A `main` rev after the relicense is the only option that is both buildable and license-clean. Reading manifests by hand missed this (D16 was written from a source survey); only the full-graph tool catches transitive declarations, so it becomes the gate rather than manual review.

**Consequences**: The pin is a `main` snapshot, so bumps need a `cargo deny check` before merge, every time. CI fails on any new copyleft crate anywhere in the graph. The Linux CI job installs gpui's system libraries (xkbcommon, wayland, fontconfig, freetype, x11-xcb, vulkan headers); macOS/Windows runners are a TODO. `.cargo/config.toml` sets `net.git-fetch-with-cli = true` because libgit2 fetched the Zed repo at ~3 MB per 10 min on this network.

---

## D18 — Utility dependencies for the UI shell (2026-09-03)

**By**: Claude (Phase 1, code review follow-up).
**Decision**: Adopt `unicode-segmentation` (grapheme-cluster boundaries for `Input` cursor motion), `percent-encoding` (decoding `DATABASE_URL` userinfo/path), and promote `url` from dev- to runtime dependency. Rule going forward: a pure-Rust, permissively licensed utility crate that is *already in the dependency graph* (here: all three arrive via gpui or tokio-postgres) may be added with a PROGRESS decisions-log row only; a crate that is **new to the graph** still needs a DECISIONS entry naming its license and why no existing dependency covers it.
**Why**: CLAUDE.md classifies "new dependency" as MAJOR. Applying a full entry to every hashing/parsing helper would bury the record in noise, while skipping it silently violates the rule. The "already in the graph" test keeps `cargo deny` as the sole license gate (D17) and adds zero new supply-chain surface.
**Consequences**: `unicode-segmentation` is the canonical grapheme library (no `unicode-width`/ICU alternatives without superseding this). `percent-encoding` decoding is applied wherever a `url::Url` component becomes a credential or identifier. This entry is the precedent for future "already in graph" additions.

---

## D19 — Driver-agnostic connection pooling with `deadpool` (2026-09-03)

**By**: Claude (Phase 1, implementing the pooling model of docs/09-database-engine.md).
**Decision**: `ConnectionService` pools `Box<dyn DriverConnection>` with the `deadpool` core crate (`managed` feature) through a Tempr `DriverManager` (`create` = `DatabaseDriver::connect`). Each `Connection` gets a **user pool** (`PoolConfig::max_size`, default 8) borrowed by `QueryService`, and a **dedicated metadata slot** (a separate 1-connection pool) borrowed only by `SchemaService`. `connect` warms one user connection eagerly. `deadpool-postgres` is removed from the workspace.
**Why**: `deadpool-postgres` pools `tokio_postgres::Client`, which sits *below* the `DatabaseDriver` abstraction (D4) — using it would make the pool PostgreSQL-specific and bypass the trait. Writing our own pool duplicates well-tested code for no gain; `deadpool`'s manager trait is small, runtime-agnostic, and lets the pool hold the trait object directly. The separate metadata pool is the simplest way to guarantee the "schema refresh never blocks behind a user query" rule without a custom slot scheduler.
**Consequences**: Borrow sites take a `PooledConnection` by value and return `Result<R, DriverError>`; the connection returns to the pool on drop (move it into the future — a closure parameter the future does not capture is returned before the body runs). `DriverConnection::is_closed` (sync, no I/O) is part of the driver trait so `recycle` evicts dead idle connections; an active ping/reconnect-with-backoff is still TODO. Pools have `wait_timeout` (30 s) and `create_timeout` (15 s) via `deadpool::Runtime::Tokio1`, and the PG driver sets a 10 s `connect_timeout`, so nothing parks forever. State and pools live in one `ConnEntry` map under one lock; a `ConnectingGuard` flips an aborted `connect` to `Failed`; a `disconnect` racing a warm-up wins. `ServiceRegistry::stop_all` runs from the GPUI `on_app_quit` hook (`gpui_compat::on_app_quit`), so quitting cancels runs (bounded, concurrent) and drains pools. Pool sizing per connection will come from the workspace connection config (`pool_max_size` in 09-database-engine's `ConnectionConfig`); today `PoolConfig` is service-wide.

---

## D20 — PostgreSQL TLS with rustls and libpq `sslmode` semantics (2026-09-03)

**By**: Claude (Phase 1, closing the last Phase 1 checklist box).
**Decision**:
1. TLS for `tempr_db_postgres` uses **rustls** through `tokio-postgres-rustls` (MIT) with the `ring` crypto provider, and trusts the **platform root store** via `rustls-native-certs`. No OpenSSL / native-tls.
2. The domain `Connection` gains `tls: TlsMode` — `Disable | Prefer | Require | VerifyCa | VerifyFull` — serialised in kebab-case and parsed from libpq spellings (`allow` → `Prefer`). Default is **`Prefer`**. `DATABASE_URL?sslmode=…` and the workspace `ConnectionConfig.tls` field (defaulting when absent) carry it.
3. Semantics follow libpq: `prefer`/`require` encrypt **without** verifying the server certificate (a custom `ServerCertVerifier` accepts any chain but still checks handshake signatures); `verify-ca`/`verify-full` verify chain **and hostname** against the platform roots. Tempr deliberately treats `verify-ca` as `verify-full`: rustls always checks the name, and the weaker mode protects nothing the stronger one does not.
4. The cancel path (`CancelToken::cancel_query`) uses the same connector as the session; `QueryService` bounds each cancel (5 s) because the TLS handshake is not covered by `connect_timeout`.
5. `prefer` keeps libpq's fallback: if the server offers TLS but the handshake fails, the driver logs a warning and reconnects in plaintext. Connectors (`Arc<ClientConfig>`) are built once per process and shared across pooled connections; the platform root store is loaded on a blocking thread.

**Why**: D2 (Rust only) and the no-system-library stance rule out OpenSSL; rustls is the standard pure-Rust stack and `ring` avoids the C toolchain `aws-lc-rs` needs. Platform roots (not a bundled Mozilla set) let corporate CAs and self-signed roots installed on the machine work without Tempr-specific configuration. libpq's mode names are what every PostgreSQL user already knows and what `psql`/connection strings emit, so inventing a Tempr vocabulary would only add translation. Defaulting to `prefer` mirrors libpq and keeps plaintext dev databases working while encrypting whenever the server allows it — the PRODUCT acceptance text since Phase 1.

**Consequences**: `Connection` literals need `tls`; older manifests deserialise with `prefer`. Client certificates, custom CA files (`sslrootcert`) and CRLs are not supported yet — tracked in TODO; until then a self-signed server can only be used with `require`/`prefer` (encrypted, unverified). Integration tests need a TLS-enabled PostgreSQL (`DATABASE_URL_TLS`); the session log records the docker command. New deps: `tokio-postgres-rustls`, `rustls`, `rustls-native-certs`, transitively `ring` — all pass `cargo deny`.

---

## D21 — Rope buffer on `ropey`, byte-offset API (2026-09-03)

**By**: Claude (Phase 2, following the recommendation in docs/10-editor.md "Rope crate choice").
**Decision**:
1. `tempr_editor::Buffer` stores text in a `ropey::Rope` (1.x, MIT/Apache-2.0). Zed's `sum_tree` is not used even though it is already in the dependency graph via gpui.
2. Every public offset is a **UTF-8 byte offset** (`Range<usize>`, `Point.column` in bytes). ropey's char indexing is an implementation detail converted at the boundary; offsets that are not char boundaries are rejected.
3. `Buffer::edit(&[(Range<usize>, &str)]) -> Result<Option<EditId>, EditError>`: ranges are expressed against the current text, validated up front (bounds, boundaries, overlap), applied highest-start-first (ties: longer range first, then later caller entries first), recorded as one undo transaction that is reverted LIFO; `Ok(None)` for a batch that changes nothing, leaving history untouched; on any error the buffer is untouched. The 10-editor sketch returned a bare `EditId`.
4. `Buffer` does not publish `BufferChanged`: it is a pure model with no `EventBus` handle; the owner publishes — this is what 10-editor's data-flow section already required ("The Buffer never publishes events directly"); the stale statements elsewhere in that document were corrected.
6. ropey is built with `default-features = false, features = ["simd"]`: line breaks are `\n` / `\r\n` only, matching what SQL tooling and the grid expect; U+2028, form feed etc. are ordinary characters.
5. The Phase 2 latency criterion is checked by an `#[ignore]`d test (`perf_10mb_insert_delete_under_1ms`) run in release on demand and recorded in PROGRESS, not by a CI benchmark (15-coding-standards: benchmarks are advisory).

**Why**: tree-sitter, `StatementRange`, `str` slicing and the result grid all speak bytes; leaking ropey's char indices would force every caller to convert and invite off-by-one bugs at multibyte characters. A `Result` on `edit` turns caller bugs into errors instead of panics inside a GPUI frame. Keeping the buffer free of the bus keeps it trivially testable and lets one buffer be driven from tests, services, or views alike. `ropey` over `sum_tree`: battle-tested standalone crate with a `str`-chunk API that feeds tree-sitter directly, versus coupling to Zed's internal structures.

**Consequences**: Measured on this machine (release, 10 MB buffer, 200 mid-document insert+delete pairs): **avg 1.73 µs, worst 12.1 µs** — the 1 ms criterion holds with three orders of magnitude to spare. `EditHistory` is unbounded for now (cap/coalescing tracked in TODO). Syntax tree and statement detection attach to `Buffer` in the next tasks; tree-sitter edits will be fed from the same recorded `Change`s.

---

## D22 — tree-sitter grammar and lazy incremental reparse (2026-09-03)

**By**: Claude (Phase 2, box 2 of the checklist).
**Decision**:
1. The SQL grammar is **`tree-sitter-sequel`** 0.3 (DerekStride's `tree-sitter-sql`, MIT; PostgreSQL-flavoured with dollar quoting, `create function` bodies, `explain`, DDL/DML), consumed through the stable `tree-sitter-language` ABI shim; the runtime is **`tree-sitter` 0.25** (the grammar's own dev-dependency line; grammar ABI 14). The alternative `tree-sitter-sql` crate (m-novikov, 0.0.2) is stale.
2. `tempr_editor::SyntaxTree` wraps parser + tree; `Buffer` owns one. Every rope change also calls `tree.edit(InputEdit)` (O(1) bookkeeping, byte + row/column positions computed from the rope) and marks the tree dirty; the **parse runs lazily** on the next read (`syntax()`, `statement_at`, `statement_ranges`, `highlights`) or an explicit `reparse()`. Parsing reads the rope's chunks directly (`parse_with_options`), never a full-text copy.
3. Statement boundaries come from the tree: each `statement`/`transaction`/`block` child of `program` (positively matched by kind), with a directly following `;` folded in; `ERROR` recovery nodes are returned as `StatementKind::Error` so an executor can refuse them; comments and stray `;` are skipped; dollar-quoted bodies and string literals are opaque to the boundary. A block or transaction is one range (inner-statement execution is a TODO). This is the statement detector — no separate hand-written scanner.
4. Syntax highlighting uses Tempr's own `queries/highlights.scm`, derived from the grammar's bundled query with the Lua `%d` classes replaced by `[0-9]` (the Rust binding evaluates `#match?` with the `regex` crate, so the original never matched numbers) and `@spell` dropped; for one node range the last matching pattern wins.
5. `Buffer::new` does not parse; the first tree read performs the full parse, later reads re-parse incrementally.

**Why**: Measured in release on a 10 MB buffer: full parse 2.0–2.7 s; incremental reparse after a one-line edit **1.6 ms** for realistic statement sizes but **~150 ms** for a dump of 180k one-line statements (tree-sitter re-walks the flat sibling list). Running the parse inside `edit` would have turned the rope's 6 µs edit into hundreds of milliseconds on such files and violated the Phase 2 "< 1 ms insert/delete" criterion; deferring it keeps typing cheap and lets the owner decide when (and later, on which thread) to parse. tree-sitter itself is the settled choice (10-editor, ADR-0003); the grammar was picked for PostgreSQL coverage, maintenance, and license.

**Consequences**: Tree readers take `&mut Buffer` (they may parse). A background/incremental-by-viewport parse strategy for pathological dumps is a TODO; so is `SyntaxTree` sharing with the semantic engine (tree-sitter `Tree` is a cheap ref-counted clone). Grammar and runtime versions are bumped together. `cc` compiles the generated `parser.c` at build time — accepted as part of the tree-sitter choice (the parser is generated, not hand-written C), consistent with D2's intent.

---

## D23 — Command catalog, palette, and configurable keybindings (2026-09-05)

**By**: Claude (Phase 2, boxes 4, 5 and 8).
**Decision**:
1. Every user action is a GPUI action type declared with `actions!` and listed exactly once in the typed catalog `tempr_ui::commands::core_commands()` — id (`CommandId` = the GPUI action name, e.g. `main_window::RunQuery`), title, category, key context, default keystrokes, and constructors for the action and its `KeyBinding`. The catalog is the keyboard-only audit: a unit test fails if any command lacks a default keystroke, and `TEMPR_LIST_COMMANDS=1` prints the effective table.
2. `tempr_services::CommandService` owns the *data*: registered `CommandContribution`s and the resolved keybinding map. Keybindings are layered lowest → highest: catalog defaults, user settings (`~/.config/tempr/settings.toml`, `[keybindings]`), workspace (`workspace.toml`, `[keybindings]`, wired when workspace open lands). Format: `"main_window::RunQuery" = ["f5", "ctrl-enter"]`, keystrokes in GPUI syntax (`ctrl-shift-p`, chords space-separated); `[]` unbinds. Invalid keystrokes are skipped with a warning, never a panic.
3. Execution stays in the UI layer: the palette emits the chosen `CommandId`; `MainWindow` builds the typed action from the catalog and dispatches it into the window (same path as a keypress), then calls `CommandService::record_executed`, which publishes `CommandExecuted { id }`.
4. The palette is `Input` + a virtualized list of `CommandService::search` hits; search is the in-house `fuzzy_match` (case-insensitive subsequence; consecutive-run and word-start bonuses, gap penalty; id fallback), no external matcher crate.

**Why**: GPUI actions already give type-safe dispatch, key contexts and bindings; inventing a parallel closure-based command runtime (as 05-services sketched) would duplicate that and drag GPUI types into the service layer (D6 forbids UI in services, and services must stay testable without a window). Keeping metadata + configuration in the service and the typed constructors in the UI splits along the existing crate boundary. `KeyBinding::new` needs a concrete action type, which is why the catalog stores monomorphized constructors rather than building actions by name. Command → keystrokes (not keystroke → command) makes an override replace *that command's* keys without silently stealing another command's key, and matches how users think ("rebind Run Query").

**Consequences**: Plugin commands (08-plugin-api `CommandContribution`) will register through the same service; their actions need a GPUI action type or a generic `PluginCommand { id }` action — decided when plugins land. `05-services.md` CommandService signatures updated to the real ones; `by_keybinding`/`execute` on the service do not exist. Per-character match highlighting in the palette and the workspace settings layer are TODO. The user-facing keybinding format is documented in 07-storage (file) and 11-gpui (catalog).

## D24 — Workspace keybindings load at startup, before workspace open (2026-09-07)

**By**: Claude (Phase 2 exit sweep), on the user's call when the sweep found box 5 unmet.

**Decision**: The binary resolves a `workspace.toml` path at startup — `TEMPR_WORKSPACE` (a workspace directory, or the manifest file itself) when set, otherwise `./workspace.toml` — reads its `[keybindings]` table, and passes it to `CommandService::set_keybinding_layers(vec![user, workspace])`. A missing manifest is the normal case (defaults + user layer only); a corrupt one is non-fatal: defaults are kept, the error is logged and shown once in the status bar, exactly as for `settings.toml`. To serve callers that need the manifest before an async runtime exists, `tempr_workspace::manifest` gains sync `parse_manifest` / `load_manifest_from` next to the async `Storage::load_manifest`; env-var reading stays in the binary, never in a library crate.

**Why**: Phase 2's exit criterion is "keybindings are configurable via the workspace format". Every piece existed — the manifest field, the layering, the tests — except a runtime that ever read a `workspace.toml`, because workspace open (connection list, recents, picker UI) is still parked. Blocking a phase on unrelated UI work, or quietly rewriting the criterion down to the user layer, both misreport where the build stands; loading the file by path satisfies the criterion as written in ~40 lines and is the same code workspace open will call later.

**Consequences**: `Storage` is no longer the only path to a manifest — the sync helpers are documented as the pre-runtime exception and must stay read-only (writes remain atomic through `Storage::save_manifest`). Layers are still resolved once at startup: editing `workspace.toml` or `settings.toml` needs a restart until live rebind lands (TODO). When workspace open arrives it replaces the path resolution, not the layering, and `TEMPR_WORKSPACE` becomes a dev knob for pointing at a workspace without the picker.

## D25 — Schema object identity is derived, not random (2026-09-08)

**By**: Claude (Phase 3 stage 2).
**Decision**: `SchemaObjectId` for a catalog object is UUIDv5 over the owning connection's id as namespace and `(kind discriminant, native_id)` as name, via `SchemaObjectId::derived`.
**Why**: a cache that cannot be diffed is a cache that must be thrown away on every refresh; kind is in the key because a packed column id can numerically equal a relation OID after OID wraparound.
**Consequences**: renumbering `SchemaObjectKind::discriminant` orphans every cache file in the wild; drivers with no stable native id hash their qualified name into the same field and get rename-as-delete-plus-insert.

## D26 — Incremental refresh diffs an `(oid, xmin)` sweep (2026-09-08)

**By**: Claude (Phase 3 stage 2).
**Decision**: The driver's fingerprint sweep is one query; `SchemaService::refresh_incremental` diffs it against the fingerprints stored with the cached snapshot and re-introspects only the affected schemas. Full refresh is the fallback in four cases: no cached fingerprints, no driver support, an object the cache has never seen, and more than 40% of objects touched.
**Why**: PostgreSQL has no change feed, event triggers would write into the user's database, and a frozen `xmin` produces a false positive (a harmless re-introspect) rather than a missed change.
**Consequences**: re-introspection is per schema, not per object, because catalog queries are shaped by schema and name while the sweep returns only OIDs. A dropped fingerprint carries only `(Table, oid)` — a `pg_class` sweep cannot say whether the relation was a table or a view — so removing it must delete both the `Table`-derived and the `View`-derived id for that oid; whichever one is actually cached is the one that goes.

## D27 — Catalog cache format is bincode behind a versioned header (2026-09-08)

**By**: Claude (Phase 3 stage 2), implementing spec decision 4.
**Decision**: `.tcat` files are a fixed header (magic `TCAT`, `u16` format version, `u16` flags, `u64` content hash) followed by `bincode` of the private `CatalogSnapshot` mirror (bincode cannot decode `SchemaSnapshot`'s internally-tagged enum directly — see Consequences). `bincode` 2.x is adopted as a dependency under the D18 rule (small, pure-Rust, already-serde-shaped). Any file whose magic, version or hash does not match is discarded and re-introspected.
**Why**: the catalog is derived data, so the cheapest safe failure mode is to throw it away; that makes format evolution a version bump rather than a migration. `bincode` needs no schema and reuses the serde derives the domain already has. Resolves OD#1 in 07-storage, which had weighed `rkyv` and an SQLite table — `rkyv`'s zero-copy win is real but unmeasured, and it buys a `SAFETY` burden before any number justifies it.
**Consequences**: a second serialization format in the tree (TOML for manifests, JSON for storage, bincode for caches). `CATALOG_FORMAT_VERSION` must be bumped on any layout change, and the load probe in the spec's §8 is the evidence that would justify revisiting the choice. `SchemaObject`'s `#[serde(tag = "kind", ...)]` JSON shape cannot round-trip through bincode's serde bridge (internally-tagged enums need `deserialize_any`, which bincode's non-self-describing `Deserializer` does not implement); `tempr_workspace::catalog` carries a private, externally-tagged mirror (`CatalogObject`/`CatalogSnapshot`) as the bincode wire shape instead, so `tempr_domain` and its JSON format are untouched. Separately, `cargo deny check` flags bincode itself as unmaintained (RUSTSEC-2025-0141: the maintainers stopped development after a harassment incident, not a code defect); `deny.toml` ignores it under this decision's own terms, to be revisited at the same §8 load probe.
