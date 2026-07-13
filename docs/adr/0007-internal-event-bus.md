# ADR-0007: Internal Event Bus

- **Status:** Accepted
- **Date:** 2026-07-13
- **Deciders:** Project brief (founding decision)

## Context
Services in a service-oriented architecture need a decoupling mechanism. Direct calls create tight coupling; no external broker is appropriate for a desktop application.

## Decision
Internal event bus using a typed enum (AppEvent) with async fan-out. Core events are exhaustively listed in the enum; plugins get a namespaced PluginEvent variant. RAII Subscription handles.

## Alternatives Considered

**Direct calls everywhere:** Tight coupling, hard to add cross-cutting concerns like logging or debugging. Rejected because it prevents transparent instrumentation and makes refactoring service boundaries risky.

**External message broker:** Absurd for a desktop app, adds process management overhead. Rejected because the single-process model of a desktop application makes an external broker unnecessary complexity.

**Trait-object events:** Loses compile-time exhaustiveness for core events. Rejected because exhaustive match on events is valuable for catching missed handlers at compile time, especially during refactors.

## Consequences
- Events carry IDs not payloads (minimal allocations pillar)
- UI subscribers receive events on main thread
- Ordering guaranteed per-publisher
- Backpressure via bounded channels with drop-or-coalesce for high-frequency events
- Event inspector for debugging is a future consideration

## Related
- [06-event-system.md](../06-event-system.md)
- [05-services.md](../05-services.md)
- [08-plugin-api.md](../08-plugin-api.md)
