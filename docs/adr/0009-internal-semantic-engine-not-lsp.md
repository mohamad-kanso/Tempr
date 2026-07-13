# ADR-0009: Internal Semantic Engine, Not LSP

- **Status:** Accepted
- **Date:** 2026-07-13
- **Deciders:** Project brief (founding decision)

## Context
Tempr needs SQL intelligence (completion, diagnostics, hover, go-to-definition). The standard approach for editor intelligence is the Language Server Protocol.

## Decision
Build an internal semantic engine that runs in-process. Use LSP concepts (requests, capabilities) as internal API shapes for familiarity, but do not use the LSP protocol or a separate LSP server process.

## Alternatives Considered

**External SQL LSP (sqls, postgres_lsp):** Process boundary adds latency on every keystroke, blocks direct access to workspace's semantic context and catalog cache, protocol constrains DB-aware features like schema-aware completion. Rejected because the latency and abstraction mismatch outweigh any reuse benefit.

**Embedding an existing LSP client:** Adds protocol overhead, limits access to internal state. Rejected because wrapping an LSP client around a custom server adds unnecessary indirection when the engine runs in the same process.

## Consequences
- Full control of completion pipeline (12-sql-intelligence.md)
- No LSP ecosystem reuse (acceptable for SQL where ecosystem is small)
- <5ms completion budget achievable via in-memory interned catalog
- Internal API can be richer than LSP (workspace context, multi-buffer analysis)
- LSP concepts kept as internal shape for contributor familiarity

## Related
- [12-sql-intelligence.md](../12-sql-intelligence.md)
- [10-editor.md](../10-editor.md)
- [ADR-0003](0003-custom-sql-editor.md)
