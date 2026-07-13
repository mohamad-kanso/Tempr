# Tempr

Native Rust Database IDE — Zed's speed + DataGrip's SQL intelligence. PostgreSQL first. Not a DBeaver clone.

## Living documentation (docs/PRODUCT.md · docs/PROGRESS.md · docs/TODO.md · docs/DECISIONS.md)

Four files that must always reflect reality. Each fact has exactly ONE home — link, never duplicate:

| File | Owns | Read it when |
|---|---|---|
| `docs/PRODUCT.md` | WHAT & WHY — features + acceptance criteria, platform constraints, out-of-scope, NFRs, Open Decisions | Before implementing or changing ANY user-facing behavior |
| `docs/PROGRESS.md` | WHERE — current status, phase checklist, decisions log (fine-grained), session log | FIRST thing every session (the status block tells you the next action) |
| `docs/TODO.md` | WHAT'S PARKED — Now / Next / Later / Ideas | When something comes up that isn't the current task |
| `docs/DECISIONS.md` | WHY (MAJOR) — the numbered cross-session record of major decisions with rationale and consequences | Before revisiting ANY settled approach, and whenever a major decision is made |

**Session start ritual**: read the PROGRESS.md status block → that is your context and next action. Skim TODO.md "Now" and the DECISIONS.md index. Open PRODUCT.md for the feature you're about to touch.

**Update triggers** (do these in the same turn/commit as the change, per the hard rule below):

| Event | Update |
|---|---|
| Task completed | PROGRESS: status block + checklist; amend today's session-log row (one row per session, not per micro-task) |
| Business behavior / requirement / scope changed | PRODUCT (feature text, constraints, or Open Decisions) + a PROGRESS decisions-log row |
| **MAJOR decision made** (architecture/stack/pattern rule, new dependency, business-rule change, blueprint deviation, closed open decision, explicit user directive) | DECISIONS.md entry (numbered, with By/Why/Consequences) in the same turn; the PROGRESS decisions-log row links `→ D<n>` instead of restating the why |
| An open decision gets resolved | Move it from PRODUCT's Open Decisions into the feature text; DECISIONS.md entry; log row links it |
| New idea or deferred work appears | TODO.md immediately, in the right section — never leave it in conversation |
| Phase completed | PROGRESS checklist + status block, "Current phase" line below, sweep TODO (promote/close items), commit |

**Consistency rules**:
- Never check a PROGRESS box without verifying (run the check commands, confirm the artifact exists) — unverified stays unchecked.
- PRODUCT's ✅/🔜 feature markers must agree with the PROGRESS checklist.
- If code and docs disagree, the code is the truth — fix the docs in the same commit.
- Keep entries terse: a decisions-log row is one line of what + why; prose belongs in PRODUCT; major rationale belongs in DECISIONS.md.
- DECISIONS.md entries are never edited or silently overturned — supersede with a new entry carrying `Supersedes: D<n>`. The one allowed edit to an old entry: add a `**Superseded by**: D<m>` line under its title.
- When the session log (or decisions log) exceeds ~30 rows, archive all but the newest ~10 to `docs/archive/PROGRESS-<year>.md` at a phase boundary — never mid-task. DECISIONS.md is never archived or trimmed.

## Project Status

Architecture/documentation phase complete; implementation not yet started. **Do not write implementation code or scaffold crates unless explicitly asked.**

## Locked Decisions

- GPUI only — no egui, Electron, Qt ([ADR-0001](docs/adr/0001-gpui-for-ui.md))
- Rust only — no other languages for app or v1 plugins ([ADR-0002](docs/adr/0002-rust-only.md))
- Custom SQL editor — never embed Monaco, CodeMirror, Scintilla ([ADR-0003](docs/adr/0003-custom-sql-editor.md))
- PostgreSQL first via driver abstraction ([ADR-0004](docs/adr/0004-postgresql-first.md))
- Workspace-first — everything scoped to a workspace directory ([ADR-0005](docs/adr/0005-workspace-first.md))
- Service-oriented — no business logic in UI ([ADR-0006](docs/adr/0006-service-oriented-architecture.md))
- Event-driven internals — services publish/subscribe ([ADR-0007](docs/adr/0007-internal-event-bus.md))
- Plugin-extensible everything from day one ([ADR-0008](docs/adr/0008-plugin-system.md))
- Internal semantic engine — no external LSP, never query DB during completion ([ADR-0009](docs/adr/0009-internal-semantic-engine-not-lsp.md))

## Docs Map

| Doc | Title |
|-----|-------|
| [01-vision.md](docs/01-vision.md) | Vision — product identity, pillars, non-goals |
| [02-architecture.md](docs/02-architecture.md) | Software Architecture — layers, dependency rules |
| [03-domain-model.md](docs/03-domain-model.md) | Domain Model — entities, vocabulary |
| [04-workspace.md](docs/04-workspace.md) | Workspace — model, lifecycle, format |
| [05-services.md](docs/05-services.md) | Services — catalog, contracts, rules |
| [06-event-system.md](docs/06-event-system.md) | Event System — bus, taxonomy, delivery |
| [07-storage.md](docs/07-storage.md) | Storage — disk layout, cache, history |
| [08-plugin-api.md](docs/08-plugin-api.md) | Plugin API — extension points, traits |
| [09-database-engine.md](docs/09-database-engine.md) | Database Engine — drivers, queries, streaming |
| [10-editor.md](docs/10-editor.md) | Editor — buffer, rope, tree-sitter |
| [11-gpui.md](docs/11-gpui.md) | GPUI — components, rendering, conventions |
| [12-sql-intelligence.md](docs/12-sql-intelligence.md) | SQL Intelligence — semantic engine, completion |
| [13-result-grid.md](docs/13-result-grid.md) | Result Grid — virtualized, streaming |
| [14-project-layout.md](docs/14-project-layout.md) | Project Layout — Cargo workspace, crate map |
| [15-coding-standards.md](docs/15-coding-standards.md) | Coding Standards — Rust, testing, review |
| [16-roadmap.md](docs/16-roadmap.md) | Roadmap — phased plan to v1 |
| [adr/](docs/adr/) | Architecture Decision Records |
| [rfc/](docs/rfc/) | RFC Process |

**Reading order:** 01 → 02 → 03 → area doc.

## Conventions

- Canonical names live in [03-domain-model.md](docs/03-domain-model.md) and [05-services.md](docs/05-services.md) — reuse, never invent parallel names
- Changes to locked decisions require an RFC ([rfc/README.md](docs/rfc/README.md)) and a superseding ADR
- Conventional Commits: `feat:`, `fix:`, `docs:`, `refactor:`

## Commands

```bash
cargo build                  # build
cargo fmt --check            # format check
cargo clippy -- -D warnings  # lint (treat warnings as errors)
cargo test                   # unit + integration tests
cargo deny check             # dependency audit (set up in Phase 0)
bash scripts/setup.sh        # install git hooks (run once after clone)
```

## Hard rules

- **No business logic in UI.** Views call services; services contain logic; services publish events; views subscribe. Absolute — see D6.
- **No blocking the UI thread.** All I/O and non-trivial computation run on async runtimes. UI thread renders and dispatches events only.
- **No DB queries during completion.** Semantic engine works from cached schema data only. < 5 ms budget is a hard real-time constraint — see D9.
- **No external processes for intelligence.** No LSP server, no sidecar, no JSON-RPC hop. In-process only — see D9.
- **No non-Rust plugins at v1.** Plugin authors write Rust crates. WASM is post-v1 — see D2, D8.
- **No mouse-only features.** Every user action must be expressible as a `Command` with a keybinding.
- **No workspace data leaves the machine without explicit user action.** No telemetry, no cloud sync.
- **Changes to locked decisions require an RFC** ([rfc/README.md](docs/rfc/README.md)) and a superseding ADR.
- **Canonical names live in docs/03-domain-model.md and docs/05-services.md** — reuse, never invent parallel names.
- **Conventional Commits**: `feat:`, `fix:`, `docs:`, `refactor:`. Commit after each phase/feature; do not batch unrelated changes.
- **Living docs** — after EVERY completed task, before ending the turn: update docs/PROGRESS.md (status block, checklist, session log). Business/scope changes also update docs/PRODUCT.md + a PROGRESS decisions-log row. New ideas or deferred work go to docs/TODO.md immediately, never left in conversation. MAJOR decisions get a docs/DECISIONS.md entry in the same turn. If code and docs disagree, fix the docs in the same commit.
- **Feature work follows branch → PR → review → merge.** Every Phase checklist item goes through: `feature-dev` skill → `git checkout -b feat/ph<N>-<slug>` → implement → `/code-review` → `gh pr create` → user approves → `gh pr merge`. No direct commits to main for feature work. Branch prefixes: `feat/ph<N>-*`, `fix/*`, `docs/*`, `chore/*`. — see D13.

## Implementation reference

When implementation begins:
- Crate layout: [docs/14-project-layout.md](docs/14-project-layout.md)
- Code standards: [docs/15-coding-standards.md](docs/15-coding-standards.md)

## Current phase

Quick pointer only — `docs/PROGRESS.md` is the source of truth. Update this line as phases complete.
Phase: 0 — complete (2026-07-13). Phase 1 — not started.
