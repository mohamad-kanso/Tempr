# Phase 3 Stage 4 — Semantic Analysis Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task.

**Goal:** The `tempr_intel::SemanticEngine`: per-statement scope trees (aliases, CTEs, subqueries) resolved against the stage-3 `CatalogCache`, producing the diagnostics model — `UnknownRelation`/`UnknownColumn`/`AmbiguousColumn` + syntax diagnostics lifted from tree-sitter `ERROR` nodes — with the no-catalog silence rule.

**Architecture:** The engine walks the pinned `tree-sitter-sequel` tree directly (no intermediate IR): one scope per statement, child scopes per `subquery` node, bindings from `FROM`/`JOIN`/`UPDATE`/`DELETE` sources and `WITH` CTEs. Column references resolve against binding columns (catalog arena indices for real relations, locally-derived name lists for CTEs and FROM-subqueries) and against the catalog only through the bindings. The analyzer runs synchronously on a `&mut Buffer` (its lazy reparse needs `&mut`); the result is cached per `SqlFileId` and validated by a new monotonic `Buffer::revision()` that moves on every text mutation *including undo/redo* — the spec's "validated by an integer compare" cannot key on `EditId` alone because `undo` restores an earlier text state while minting no new id.

**Tech Stack:** Rust 1.97.1, `tempr_editor` (Buffer/SyntaxTree), `tree-sitter` reads, existing `parking_lot` (RwLock over `Arc<CatalogCache>`, spec §5's arrangement), existing `tempr_domain`/`tempr_intel::CatalogCache`.

**Spec:** [2026-09-07-phase3-sql-intelligence-design.md](../specs/2026-09-07-phase3-sql-intelligence-design.md) §5.1 (pipeline + severity rules), §7 (error handling), §8 (unit tests), §9 stage 4.

## Global Constraints

- **Branch, PR, review — never commit to main.** Branch: `feat/ph3-semantic-analysis`.
- **Every task ends green**: `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`.
- **No new third-party dependencies.** `parking_lot`, `tree-sitter`, `tempr_editor` are all in the graph (add `tempr_editor` as a normal dependency of `tempr_intel` — the layout doc's intelligence layer already groups them).
- **No DB queries, no I/O, no allocation of catalog data on the request path** beyond the returned own-strings.
- **AmbiguousColumn = Warning; UnknownRelation/UnknownColumn/SyntaxError = Error** (spec decision 8). Severity drives stage-6 rendering.
- **No-catalog silence**: with an empty catalog (no connection yet, introspection failed), the analyzer emits *no* unknown-relation/unknown-column/ambiguous diagnostics — syntax diagnostics still show (spec §5.1 "Unknown catalog guard").
- **A statement whose parse contains `ERROR` nodes still resolves what it can**; no semantic diagnostics for ranges covered by `ERROR` fragments (spec §7 row 6).
- Canonical names: `SemanticEngine`, `Analysis`, `Scope`, `Binding`, `ColumnRef`, `Diagnostic`, `DiagnosticKind`, `Severity`, `analyze`, `set_catalog`, `Buffer::revision`.
- No unwrap/expect in library code; tests may.

---

## Task 0 (done): Grammar probe

Scratch probe results (2026-10-21, deleted before committing), pinned `tree-sitter-sequel` 0.3 via the editor's crate (grammar ABI 14):

**What the grammar exposes well** — enough for scope extraction as designed:
- Relations: `relation` items under `from` carry an `object_reference` (single identifier; schema qualification is a multi-part object_reference) plus a trailing `identifier` alias. Join flavors are distinct nodes: `join`, `cross_join`, `lateral_join` (lateral wraps a `subquery`).
- Column refs: `field` = bare (`identifier`) or qualified (`object_reference + '.' + identifier`); aliases on terms are `field + keyword_as + identifier`.
- CTEs: sibling `cte` nodes under the statement — `identifier` (name), an optional parenthesized column list (`( identifier, identifier )`), `keyword_as`, then a parenthesized `statement` body. Later CTEs see earlier ones (verified with a two-hop chain and `WITH RECURSIVE`).
- Subqueries: explicit `subquery` node in `exists`, in a FROM `relation`, and in `lateral_join` — child scoping works directly.
- Set operations: `set_operation` node with two `select` children (`order_by`/`limit` attach to the last select; scope-wise both sides share one statement scope).
- Function calls are `invocation` (with `filter`), `case`, `update` (relation + alias + `from` joins), `drop_table` (object_reference), `transaction`.

**Grammar shallowness found (diagnostics must tolerate it):**
- `delete … using`, `insert … on conflict (…)` and `limit $3 offset $4` produce `ERROR` fragments.
- `select distinct on (u.id) …` mis-parses (`u.*` becomes `ERROR`).
- `u.*` → `term → all_fields` with **no** qualifier child; qualified-star handling slices raw text by byte range instead of trusting children.
- Bare `select 1 where exists (…)` (no FROM before) mis-parses; `rollback` after `commit` in one program is an `ERROR` statement.

→ None of these block scope extraction; they are parked in TODO as grammar-version follow-ups. The plan proceeds.

---

## File Structure

| File | Responsibility in this stage |
|---|---|
| `crates/tempr_intel/src/analysis.rs` | **New.** `Analysis`, `Scope`, `Binding`, `ColumnRef`, `Diagnostic`, `DiagnosticKind`, `Severity` — the stage-4 model types |
| `crates/tempr_intel/src/semantic.rs` | **New.** The engine: catalog `RwLock<Arc<CatalogCache>>`, analysis cache, `analyze` (tree walk → scopes + diagnostics) |
| `crates/tempr_editor/src/buffer.rs` | `revision()` accessor + the revision counter on every text mutation |
| `docs/12-sql-intelligence.md`, `docs/PROGRESS.md`, `docs/TODO.md` | Living docs per CLAUDE.md |

---

## Task 1: `Buffer::revision`

**Why:** the engine caches analyses per file and must detect a buffer whose *current text* changed — including via undo/redo, where `next_edit` does not move (verified in the probe session: `undo()` returns the original `EditId` and mints no new one).

- [ ] Failing test first: `revision_starts_at_zero_and_dances_with_undo_redo` — new buffer → 0; every successful `edit` → +1; `undo`/`redo` → +1 each; failed edits → unchanged.
- [ ] Implement: `revision: u64` field incremented inside the one internal place where `next_edit` is bumped and in the undo/redo state-change paths; `pub fn revision(&self) -> u64`.
- [ ] Green, fmt/clippy/tests → commit `feat(editor): monotonic Buffer revision that moves on undo/redo`.

## Task 2: Analysis model types

- [ ] `analysis.rs` with the types above (owned strings, copyable indices; `ColumnRef::Catalog(u32)` arena idx vs `ColumnRef::Local { name, data_type: Option<Box<str>> }` for CTE/subquery-derived columns), `Severity` order helper `errors_before_warnings` used by stage 6's `NextDiagnostic`.
- [ ] Test: severity ordering helper; a binding's columns resolve lookup names case-folded (helper on `Binding`).
- [ ] Green → commit `feat(intel): analysis model — scopes, bindings, diagnostics`.

## Task 3: Scope collection walk

**Interfaces:**
```rust
impl SemanticEngine {
    pub fn new(catalog: Arc<CatalogCache>) -> Self;              // empty catalog = silence rule
    pub fn set_catalog(&self, catalog: Arc<CatalogCache>);
    pub fn catalog(&self) -> Arc<CatalogCache>;
    pub fn analyze(&self, file: SqlFileId, buffer: &mut Buffer) -> Arc<Analysis>; // re-runs when revision changed, else cached Arc
}
```
- [ ] Failing tests (placeholder driver): an aliased-join query yields a statement scope with two bindings (`u`→users, `o`→orders) resolved against a synthetic `CatalogCache`; the catalog never touches the database (pure types, no driver crate in the graph).
- [ ] Walk: statement ranges from `Buffer::statement_ranges`; per statement — CTE registration (name, explicit column list, else derived from the body's top select list), FROM/JOIN/UPDATE relation binding with alias; subqueries as child scopes carrying ancestor bindings + visible CTEs; `lateral_join` sees left-side bindings; set-operation selects share the statement scope.
- [ ] Columns of a binding: for catalog relations (exact + folded name match, `SearchPath`-shaped: unqualified reference resolves `schema` empty → any schema in cache), from the arena `columns_of`; for FROM-subqueries from the inner select list (alias or field name); for CTEs explicit list > derived names > local-unknown.
- [ ] Green → commit `feat(intel): scope collection — aliases, CTEs, subqueries over the parsed tree`.

## Task 4: Reference resolution + diagnostics

- [ ] Failing tests per spec §8: bare column unique / ambiguous (warning, names both candidates) / unknown (error) inside and across joined tables; qualified `a.col` correct; `q.unknown` → `UnknownRelation` at the qualifier's range; `a.unknowncol` → `UnknownColumn`; CTE chain (`b` sees `a`); correlated subquery resolves outer alias; FROM-subquery alias resolves; `select *`/`u.*` produce no diagnostic; an unknown schema-qualified relation is `UnknownRelation`.
- [ ] Resolution: only `field` nodes; unseen-at-this-stage node kinds (`case`, `invocation`, literals, parameters) are skipped silently; identifiers fold to lower case; quoted-identifier nuance handled by falling back to exact compare among folded candidates.
- [ ] Syntax diagnostics: `DiagnosticKind::SyntaxError` at each `ERROR` fragment's range, generic message; semantic diagnostics suppressed for ranges inside any `ERROR` fragment.
- [ ] No-catalog guard: `catalog.stats().relations == 0` (`CatalogCache` says empty) → no semantic diagnostics at all.
- [ ] Green → commit `feat(intel): reference resolution with severity-ordered diagnostics`.

## Task 5: Engine caching + living docs

- [ ] Cache path: `(SqlFileId, revision)` compare; same revision returns the cached `Arc<Analysis>`; cache is a plain `Mutex<HashMap>` — bounded growth is stage 6's concern (service evicts on buffer close).
- [ ] `docs/12-sql-intelligence.md`: shipped engine API replaces the sketch (spec §11 impact); note the revision-based cache validation (corrects the spec's EditId claim → add the PROGRESS decisions row).
- [ ] `docs/TODO.md`: park the four grammar-ERROR gaps (`delete…using`, `on conflict`, `limit $N`, `distinct on`) as grammar-version follow-ups; note star-expansion deferred to stage 5 (first consumer).
- [ ] `docs/PROGRESS.md`: status block, checklist untouched (stage 4 is en route to the "semantic analyzer" box — check it only after this stage's tests exist; **this stage completes that box**, mark it), decisions-log + session-log rows; probe/latency numbers n/a here.
- [ ] Commit `docs: stage 4 living-docs updates`.

## Verification (stage exit)

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo test --release -p tempr_intel -- --ignored   # stage 3 probe still green
```

PR on `feat/ph3-semantic-analysis`; review; user merges.
