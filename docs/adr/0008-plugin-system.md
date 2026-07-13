# ADR-0008: Plugin System

- **Status:** Accepted
- **Date:** 2026-07-13
- **Deciders:** Project brief (founding decision)

## Context
Tempr must be extensible. The project brief mandates extensibility for drivers, completion providers, themes, commands, panels, result renderers, and AI providers.

## Decision
Plugin system from day one. Core features are implemented as static plugins using the same Plugin API, ensuring the API is battle-tested ("eat your own dogfood"). Plugins register capabilities via PluginContext; they never receive ServiceRegistry access.

## Alternatives Considered

**Hardcode features and add plugins post-v1:** API would ossify around implementation details, making the plugin API fragile. Rejected because retrofitting a plugin API onto a codebase that never used one leads to leaky abstractions and breaking changes.

**Dynamic loading from day one:** Premature optimization, adds WASM/dylib complexity before the API surface is stable. Rejected because the API needs to stabilize through real usage before the cost of dynamic loading is justified.

## Consequences
- Seven extension-point traits defined (08-plugin-api.md)
- Core features validate the API
- Plugin crate pattern (e.g., tempr_db_postgres) proven early
- API stability enforced by internal usage
- Dynamic loading (WASM component model) is a future evolution path

## Related
- [08-plugin-api.md](../08-plugin-api.md)
- [05-services.md](../05-services.md)
- [ADR-0002](0002-rust-only.md)
