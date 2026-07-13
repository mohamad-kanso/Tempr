# ADR-0004: PostgreSQL First

- **Status:** Accepted
- **Date:** 2026-07-13
- **Deciders:** Project brief (founding decision)

## Context

Tempr is a database IDE. It must decide which database engine(s) to support in v1. The decision affects the scope of the driver abstraction, the SQL intelligence engine, and the time to a usable product. Target users are developers working with production databases, which strongly biases toward PostgreSQL.

## Decision

Ship v1 with PostgreSQL support only. Design the driver abstraction layer to support N engines but implement only PostgreSQL in the first release.

## Alternatives Considered

**Multi-engine from day one** — Supporting PostgreSQL, MySQL, SQLite, and others simultaneously from v1. This spreads the intelligence engine effort thin across multiple SQL dialects, complicates driver testing, and delays shipping a usable product. Each engine introduces unique metadata schemas, type systems, and protocol quirks that require dedicated tuning. Rejected due to scope dilution and delayed product delivery.

**SQLite first** — Starting with SQLite as the initial engine. SQLite is simpler to implement but is a weaker fit with the target user persona — developers working with production databases. SQLite lacks the metadata richness (system catalogs, extensions, roles) that Tempr's intelligence engine is designed to leverage. Rejected due to misalignment with target users.

## Consequences

- Driver abstraction is validated by adding a second engine later as a focused integration test (see `09-database-engine.md`).
- SQL intelligence is tuned for PostgreSQL dialect initially, giving a deep and polished experience for the primary audience.
- PostgreSQL-specific features can be leveraged early: `LISTEN/NOTIFY`, `pg_catalog`, `pg_stat_*` views.
- The second engine integration becomes a concrete test of the abstraction's extensibility.

## Related

- [09-database-engine.md](../09-database-engine.md)
- [01-vision.md](../01-vision.md)
