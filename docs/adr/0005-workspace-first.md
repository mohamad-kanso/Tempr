# ADR-0005: Workspace-First

- **Status:** Accepted
- **Date:** 2026-07-13
- **Deciders:** Project brief (founding decision)

## Context

Tempr must decide how to organize user data and application state. The model chosen affects portability, git-friendliness, per-project configuration, and the overall user experience. The architecture must support multiple database projects without coupling state to a global configuration.

## Decision

Workspace-first architecture. A workspace is a directory on disk containing connections, SQL files, history, layout, settings, cache, and indexes. The app always operates within exactly one workspace at a time.

## Alternatives Considered

**Connection-centric model** — A DBeaver-style approach where connections are the primary organizational unit and files are secondary to them. This model does not map well to git repositories, is not portable across machines, and scatters project state across global application directories. It assumes a single-user, single-machine context and does not support project-level database settings. Rejected due to poor git-friendliness and lack of portability.

**Global single config** — A single application-wide configuration file for all database connections and preferences. This does not support per-project database settings and does not scale to users working across multiple database projects simultaneously. There is no way to associate specific SQL files, history, or layout with a particular project. Rejected due to inability to scope state per-project.

## Consequences

- Everything is scoped to a workspace directory → git-friendly, portable, human-inspectable (see `04-workspace.md`, `07-storage.md`).
- Workspace format is versioned to support migration across Tempr releases.
- Settings layering: application defaults < user preferences < workspace-specific overrides.
- Welcome screen handles the zero-workspace state, guiding users to create or open a workspace.
- Workspace contents can be committed to a repository, enabling team sharing of connections, SQL files, and layout.

## Related

- [04-workspace.md](../04-workspace.md)
- [07-storage.md](../07-storage.md)
- [03-domain-model.md](../03-domain-model.md)
