# Architecture Decision Records (ADRs)

An Architecture Decision Record is an immutable record of a significant architectural decision. It captures the context, rationale, and consequences of a decision so that future contributors understand **why** things are the way they are, not just **how** they work.

ADRs are never edited after acceptance. If a decision must be reversed or changed, a new ADR is written that supersedes the old one. The original record remains intact.

## When Is an ADR Required?

An ADR must be written whenever any of the following occurs:

- A new external dependency is introduced that carries meaningful lock-in.
- A layer boundary is added, removed, or changed.
- A protocol or data format decision is made (wire format, serialization, file format, etc.).
- A decision contradicts an existing accepted ADR — the new ADR must explicitly supersede the old one.

## Numbering

ADRs are numbered sequentially starting at 0001. Numbers are never reused, even if an ADR is superseded. The superseded record retains its original number and status.

## Status Lifecycle

```
Proposed  →  Accepted
                ↓
         Superseded (by ADR-NNNN)
```

- **Proposed** — under discussion; not yet decided.
- **Accepted** — decided and in effect.
- **Superseded** — no longer the active decision, replaced by a newer ADR.

## Index

| ADR   | Title                              | Status     | Date       |
|-------|-------------------------------------|------------|------------|
| 0001  | GPUI for UI                         | Accepted   | 2026-07-13 |
| 0002  | Rust Only                           | Accepted   | 2026-07-13 |
| 0003  | Custom SQL Editor                   | Accepted   | 2026-07-13 |
| 0004  | PostgreSQL First                    | Accepted   | 2026-07-13 |
| 0005  | Workspace-First                     | Accepted   | 2026-07-13 |
| 0006  | Service-Oriented Architecture       | Accepted   | 2026-07-13 |
| 0007  | Internal Event Bus                  | Accepted   | 2026-07-13 |
| 0008  | Plugin System                       | Accepted   | 2026-07-13 |
| 0009  | Internal Semantic Engine, Not LSP   | Accepted   | 2026-07-13 |

> **Note:** ADRs 0001–0009 record decisions locked by the project brief. They are accepted as foundational and should not be superseded without compelling reason.
