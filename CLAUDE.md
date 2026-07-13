# Tempr

Native Rust Database IDE — Zed's speed + DataGrip's SQL intelligence. PostgreSQL first. Not a DBeaver clone.

## Project Status

Architecture/documentation phase. `docs/` is the source of truth. **Do not write implementation code or scaffold crates unless explicitly asked** — the brief locks this.

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

## Future Implementation

When implementation begins:
- Crate layout follows [14-project-layout.md](docs/14-project-layout.md)
- Code standards follow [15-coding-standards.md](docs/15-coding-standards.md)
