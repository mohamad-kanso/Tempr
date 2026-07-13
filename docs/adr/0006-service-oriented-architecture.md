# ADR-0006: Service-Oriented Architecture

- **Status:** Accepted
- **Date:** 2026-07-13
- **Deciders:** Project brief (founding decision)

## Context
Tempr needs an internal architecture that supports testability, plugin extensibility, and clean separation between UI and business logic.

## Decision
Service-oriented architecture. All business logic lives in services. UI components never contain business logic. Services communicate via events for decoupling, direct async calls for request/response.

## Alternatives Considered

**MVC monolith:** UI and logic co-located, hard to test without rendering, plugin extensibility limited. Rejected because testability and extensibility are first-class requirements in the project brief.

**Actor framework like actix:** Adds runtime overhead, learning curve, complexity disproportionate for a desktop app. Rejected because the concurrency model of a desktop application does not require actor-based isolation; async tasks on a shared runtime suffice.

## Consequences
- Service registry + typed contracts (02-architecture.md, 05-services.md)
- Services are testable without UI
- Plugins can interact with services through well-defined APIs
- 10 canonical services with clear boundaries
- Overhead of maintaining service interfaces

## Related
- [02-architecture.md](../02-architecture.md)
- [05-services.md](../05-services.md)
- [ADR-0007](0007-internal-event-bus.md)
