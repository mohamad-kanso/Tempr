# ADR-0003: Custom SQL Editor

- **Status:** Accepted
- **Date:** 2026-07-13
- **Deciders:** Project brief (founding decision)

## Context

Tempr needs a text editor component for writing and executing SQL. The editor must support syntax highlighting, code completion, statement-level execution, and SQL-specific UX patterns. The project brief mandates building a custom editor rather than embedding a third-party widget, to maintain full control over the editing experience and performance characteristics.

## Decision

Build a custom SQL editor from scratch using rope-based buffers and tree-sitter for incremental parsing. No embedded editor widgets are used.

## Alternatives Considered

**Monaco** — The editor powering VS Code. Monaco requires a web view or Electron shell to render, which directly violates the native performance pillar. It brings a massive JavaScript runtime dependency and is not designed for GPU-accelerated native rendering. Rejected due to web view requirement and memory overhead.

**CodeMirror** — Another web-based editor framework. Like Monaco, it requires a web view layer and cannot render natively on the GPU pipeline. It carries the same performance and architectural violations. Rejected due to web view requirement.

**Scintilla** — A C/C++ editing component with dated rendering. Scintilla's rendering pipeline is not designed for modern GPU-accelerated UIs. It introduces a C++ dependency and does not integrate cleanly with GPUI's compositing model. The theming and extensibility model is limited compared to what Tempr requires. Rejected due to C++ dependency, dated rendering, and misalignment with GPU pipeline.

## Consequences

- Significant engineering investment in editor infrastructure (see `10-editor.md`).
- Total control over SQL-specific UX: statement detection, gutter run buttons, keyword casing, and formatting.
- Performance aligned with native pillars — no web view, no JavaScript runtime.
- No LSP protocol dependency for syntax features; tree-sitter provides incremental parsing directly.
- Editor infrastructure becomes a reusable foundation for any future text editing needs.

## Related

- [10-editor.md](../10-editor.md)
- [12-sql-intelligence.md](../12-sql-intelligence.md)
- [ADR-0009](0009-internal-semantic-engine-not-lsp.md)
