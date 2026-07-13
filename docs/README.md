# Tempr Architecture Documentation

Tempr is a native Rust Database IDE — Zed's speed, UX, and keyboard-first workflow combined with DataGrip's SQL intelligence. PostgreSQL first. This is *not* a DBeaver clone.

## Doc Map

| File | Title | Summary |
|------|-------|---------|
| [01-vision.md](01-vision.md) | Vision | Product identity, pillars, non-goals, and user journey |
| [02-architecture.md](02-architecture.md) | Software Architecture | Top-level layers, dependency rules, service/event topology |
| [03-domain-model.md](03-domain-model.md) | Domain Model | Core entities, relationships, canonical vocabulary |
| [04-workspace.md](04-workspace.md) | Workspace | Workspace-first model, lifecycle, on-disk format |
| [05-services.md](05-services.md) | Services | Service catalog, contracts, rules, lifecycle |
| [06-event-system.md](06-event-system.md) | Event System | EventBus, AppEvent taxonomy, delivery semantics |
| [07-storage.md](07-storage.md) | Storage | Disk layout, cache, history, secret handling |
| [08-plugin-api.md](08-plugin-api.md) | Plugin API | Extension points, traits, plugin lifecycle |
| [09-database-engine.md](09-database-engine.md) | Database Engine | Driver abstraction, PostgreSQL, query execution, streaming |
| [10-editor.md](10-editor.md) | Editor | Custom SQL editor: buffer, rope, tree-sitter, commands |
| [11-gpui.md](11-gpui.md) | GPUI | GPUI usage, component library, rendering conventions |
| [12-sql-intelligence.md](12-sql-intelligence.md) | SQL Intelligence | Internal semantic engine, completion, diagnostics |
| [13-result-grid.md](13-result-grid.md) | Result Grid | Virtualized, streaming result display |
| [14-project-layout.md](14-project-layout.md) | Project Layout | Cargo workspace and crate map |
| [15-coding-standards.md](15-coding-standards.md) | Coding Standards | Rust standards, testing, review, commit conventions |
| [16-roadmap.md](16-roadmap.md) | Roadmap | Phased implementation plan to v1 |
| [adr/](adr/) | Architecture Decision Records | Immutable records of significant decisions |
| [rfc/](rfc/) | RFC Process | Proposal process for substantial changes |

## Reading Order

New contributors should read in this order:

1. [01-vision.md](01-vision.md) — understand what we are building and why
2. [02-architecture.md](02-architecture.md) — the top-level system view
3. [03-domain-model.md](03-domain-model.md) — canonical vocabulary
4. Then read the doc(s) relevant to your area of work

## Document Conventions

Every numbered architecture document (01–16) contains these sections:

- **Purpose** — why this document exists
- **Responsibilities** — what it governs
- **Design Rationale** — why this design, alternatives considered
- **Interfaces** — illustrative Rust trait/struct signatures (contracts, not implementation)
- **Data Flow** — how information moves through this subsystem
- **Mermaid diagrams** — at least one per document
- **Future Considerations** — planned evolution beyond v1
- **Open Questions** — unresolved design decisions
- **Related Documents** — cross-references to other docs

Interface sketches use fenced Rust code blocks. They document contracts and are not implementation code.

ADRs and RFCs follow their own templates and are not subject to the 8-section format.

## Status

Pre-implementation phase. These documents are the source of truth for the project architecture.

Changes to locked decisions require an RFC (see [rfc/README.md](rfc/README.md)) and a superseding ADR.
