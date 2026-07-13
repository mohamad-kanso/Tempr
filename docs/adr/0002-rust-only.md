# ADR-0002: Rust Only

- **Status:** Accepted
- **Date:** 2026-07-13
- **Deciders:** Project brief (founding decision)

## Context

Tempr needs to decide on language constraints for the entire stack, including the application core, libraries, and the plugin system. The choice affects build complexity, runtime safety, plugin author experience, and long-term maintainability. The project brief mandates Rust as the primary language.

## Decision

All code — application, libraries, and v1 plugins — is written in Rust. No other languages are permitted in the codebase.

## Alternatives Considered

**Core in Rust with TypeScript/Lua scripting for plugins** — This approach splits the toolchain into two language ecosystems and introduces a scripting runtime dependency. The scripting layer would introduce garbage collection pauses, which violates the performance pillar. The FFI boundary between Rust and the scripting language adds complexity, safety concerns, and maintenance burden. The dual-language approach also fragments the contributor community. Rejected due to split toolchain complexity, runtime dependency, GC pauses, and FFI boundary risks.

## Consequences

- Plugin authors must write Rust (see `08-plugin-api.md`), raising the barrier to entry but guaranteeing safety.
- No garbage collection pauses anywhere in the application or plugin runtime.
- Single toolchain, single build system — Cargo handles everything.
- Smaller initial plugin community but stronger correctness guarantees.
- No FFI safety concerns between application and plugin code.

## Related

- [01-vision.md](../01-vision.md)
- [08-plugin-api.md](../08-plugin-api.md)
- [ADR-0008](0008-plugin-system.md)
