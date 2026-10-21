# Phase 3 Stage 5 — Completion Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task.

**Goal:** `SemanticEngine::complete` — context-aware SQL completion over the stage-4 analysis and the stage-3 `CatalogCache`: `CompletionRequest`/`CompletionItem`, candidates from scope bindings + `CatalogCache::prefix_search` + keywords, deterministic ranking, a bounded top-N result, and the < 5 ms latency probe on a 10 000-object catalog.

**Architecture:** Completion reads the same `Analysis` stage 4 caches (scope trees, bindings, CTEs) and the immutable catalog; it never touches the tree-sitter tree for clause detection, because the stage-5 probe below shows incomplete SQL parses trailing clause keywords into top-level `ERROR` siblings. Context classification is therefore a small forward lexer over the current statement's text (comments, strings and dollar-quotes understood) plus a backward word/qualifier scan at the cursor. Candidates are generated per context (columns in scope, relations/functions from the catalog, CTEs, schemas, keywords), scored with one integer (context weight → match quality → shorter label → label), deduped, and kept in a bounded 200-item top-N heap. The hot path allocates only the returned item strings.

**Tech Stack:** Rust 1.97.1, `tempr_editor::Buffer` (text + revision), `tempr_intel` stage-3/4 model (`CatalogCache`, `Analysis`, `Scope`, `Binding`, `ColumnRef`), `tempr_domain::text::fuzzy_match` (the palette matcher, moved to the domain crate so both the palette and ranking share one implementation without a `services → intel` edge).

**Spec:** [2026-09-07-phase3-sql-intelligence-design.md](../specs/2026-09-07-phase3-sql-intelligence-design.md) §5.2 (request/item, context table, ranking), §7 (no-catalog behaviour), §8 (completion tests + performance), §9 stage 5.

## Global Constraints

- **Branch, PR, review — never commit to main.** Branch: `feat/ph3-completion`.
- **Every task ends green**: `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`.
- **No new third-party dependencies.**
- **No I/O, no DB queries, no catalog allocation on the request path** — candidates borrow from the arena; only returned `CompletionItem`s own strings.
- **Bounded output**: at most `COMPLETION_LIMIT = 200` items, deterministic order (score desc, label asc).
- **No catalog → keywords only** (spec §7 row 1); never a semantic candidate from an empty catalog.
- **Auto-open rule** (spec decision 5): with `explicit == false`, return nothing unless one identifier character or a qualifier dot precedes the cursor. `ctrl-space` sets `explicit`.
- Canonical names: `CompletionRequest`, `CompletionItem`, `CompletionKind`, `CompletionContext`, `complete`, `CatalogCache::relations_in_schema`.
- No unwrap/expect in library code; tests may.

---

## Task 0 (done): Context probe

Scratch probe (2026-10-21, deleted before committing) parsed 15 cursor shapes with the pinned `tree-sitter-sequel` 0.3 and dumped `root.to_sexp()` + the leaf/parent chain at the cursor:

- `descendant_for_byte_range(offset, offset)` at EOF returns `program`, never a leaf: tree-sitter ranges are half-open. Any tree lookup must use `offset.saturating_sub(1)..offset` or fall back to text.
- **Trailing incomplete clauses leave the statement**: `select * from ` → `(program (statement …) (ERROR (keyword_from)))`; same for `where`, `join`, and `insert into `. Clause detection from the tree is therefore unreliable exactly when completion runs.
- A partially typed word parses fine inside a clause: `select * from us` → `relation → object_reference → identifier`.
- `u.` and `public.` produce `ERROR` (in a WHERE the qualifier is dropped: `field name: (identifier)` only); `select u. from users u` errors out the FROM clause entirely.
- `select count(|)` parses as `invocation` with no argument node; `with c as (select 1) select  from c` parses as expected.
- `select * from users u, ` → the FROM clause survives, a trailing `ERROR` sibling appears.

→ **Decision:** classify context from text tokens, not tree nodes; use the tree only through the already-built `Analysis` for scope lookup. The probe's exact SQL strings become the classifier's unit-test table.

---

## File Structure

| File | Responsibility in this stage |
|---|---|
| `crates/tempr_domain/src/text.rs` | **New.** `fuzzy_match`/`FuzzyMatch` moved from `tempr_services::fuzzy` (one implementation for palette + completion ranking) |
| `crates/tempr_services/src/fuzzy.rs` | **Deleted.** `tempr_services` re-exports the domain matcher; `command.rs` imports change |
| `crates/tempr_intel/src/completion.rs` | **New.** Model types, lexer/classifier, candidate generation, ranking, top-N, tests |
| `crates/tempr_intel/src/analysis.rs` | `Analysis::scope_chain(offset)` (innermost-first path for completion/hover) |
| `crates/tempr_intel/src/catalog.rs` | `relations_in_schema(schema)` for schema-dot completion |
| `crates/tempr_intel/src/semantic.rs` | `SemanticEngine::complete` wiring |
| `crates/tempr_intel/src/probe.rs` | Completion latency leg on the 10 000-object synthetic catalog |
| `docs/12-sql-intelligence.md`, `docs/14-project-layout.md`, `docs/PROGRESS.md`, `docs/TODO.md` | Living docs per CLAUDE.md |

---

## Task 1: Shared fuzzy matcher

- [ ] Move `crates/tempr_services/src/fuzzy.rs` → `crates/tempr_domain/src/text.rs` verbatim (module doc reworded to name both consumers); `tempr_domain::text::{fuzzy_match, FuzzyMatch}` exported.
- [ ] `tempr_services`: drop `pub mod fuzzy`, re-export `pub use tempr_domain::text::{FuzzyMatch, fuzzy_match};`, point `command.rs` at the re-export; delete the old file.
- [ ] Green (existing palette tests keep passing) → commit `refactor(domain): share the fuzzy matcher between palette and completion ranking`.

## Task 2: Completion model + scope lookup

- [ ] `completion.rs` model types:
  ```rust
  pub struct CompletionRequest<'a> { pub file: SqlFileId, pub buffer: &'a mut Buffer,
                                     pub offset: usize, pub explicit: bool }
  pub struct CompletionItem { pub label: String, pub kind: CompletionKind, pub detail: String,
                              pub insert: String, pub replace: Range<usize>, pub score: i32 }
  pub enum CompletionKind { Keyword, Table, Column, Schema, Alias, Cte, Function, Snippet, Plugin }
  pub enum CompletionContext { StatementStart, Relation, Expression, Alias,
                               Qualifier(Vec<String>) }
  ```
  (`&mut Buffer` mirrors stage 4's documented deviation: the analysis cache needs a lazy reparse. `replace` is the byte range of the word under the cursor; `insert` is what the editor writes — equal to `label` today, separate for stage-6 snippets.)
- [ ] `Analysis::scope_chain(offset) -> Vec<&Scope>` — outermost→innermost scopes containing `offset`, children preferred; empty when no scope contains it.
- [ ] Tests: nested subquery chain order; boundary offset prefers the later statement; offset outside every scope is empty.
- [ ] Green → commit `feat(intel): completion model and scope-chain lookup`.

## Task 3: Context classifier

- [ ] Forward lexer over the statement prefix: whitespace, `--` line comments, nested `/* */` block comments, `'…''…'` strings, `$tag$…$tag$` bodies, `"quoted identifiers"`, words (letters/digits/`_`/`$`), `.`, `,`, parens, operators. Returns tokens; classification reads the tail only.
- [ ] Backward cursor scan: replacement word (`identifier` chars), optional qualifier path before a dot (whitespace tolerated, up to three parts, quoted part allowed).
- [ ] Classifier table (probe SQL + spec §5.2):
  | Input at cursor | Context |
  |---|---|
  | empty buffer, `sel`, after `with c as (…) ` | `StatementStart` |
  | `select * from `, `… from us`, `… join `, `insert into `, `update `, `… from a, ` | `Relation` |
  | `select `, `where `, `on `, `= 'x' `, `count(` | `Expression` |
  | `… as ` | `Alias` (no candidates) |
  | `select u.`, `select public.`, `where u.` | `Qualifier(["u"])` / `Qualifier(["public"])` |
- [ ] Tests: one per row + comment/string skipping (`select 'from ' , ` stays expression; `-- from\nselect ` stays statement start).
- [ ] Green → commit `feat(intel): completion context classifier over the statement text`.

## Task 4: Candidate generation

- [ ] `CatalogCache::relations_in_schema(schema)` — folded compare, sorted by (name, arena index); test exact/folded/missing schema.
- [ ] Visible frames at the cursor: `scope_chain` innermost-first; when the chain has no bindings/CTEs (trailing `ERROR` sibling — probe finding) fall back to the nearest preceding top-level scope.
- [ ] Generation per context:
  - **Relation**: CTEs (own→outer, `Cte`), catalog relations via `prefix_search(prefix, 200)` filtered to `Table`/`View`, schemas matching the prefix.
  - **Expression**: columns of every visible binding (bare when the folded name is unique; `alias.col` for every binding when it repeats — spec §5.2), relations, functions (signature detail), keywords, binding aliases.
  - **Qualifier(path)**: binding/CTE by folded qualifier → its column list (`Catalog` arena entries or `Local`); else schema → `relations_in_schema`; else nothing.
  - **StatementStart**: the static statement-keyword set (server keywords are unordered and include non-statement words); **Alias**: nothing.
  - Keywords: server list when loaded (uppercased), static fallback when the catalog is empty.
- [ ] Tests: relation list contains CTE before tables; `u.` returns exactly the binding's columns; `public.` returns that schema's relations; ambiguous `id` yields qualified items and no bare `id`; function signature detail; empty catalog → keywords only; alias context empty; auto-open gating.
- [ ] Green → commit `feat(intel): completion candidates from scopes, catalog and keywords`.

## Task 5: Ranking + bounded top-N

- [ ] One integer score: `context_weight * 1000 + match_quality * 4 - label.len()`, `match_quality` = exact prefix 400 > folded prefix 300 > `fuzzy_match` score (spec §5.2); context weights: column 500, qualified column 480, CTE 470, relation 450, function 300, relation-in-expression 250, alias 150, keyword 100.
- [ ] Dedup by `(label, kind)` keeping the highest score; bounded `BinaryHeap<Reverse<Ranked>>` (N = 200, worst-of kept set evicted); final order score desc, label asc.
- [ ] Tests: exact prefix beats folded beats subsequence; a column beats a keyword at equal match; result capped at 200 with the best candidates surviving; equal scores sort by label (stable); dedup keeps one item.
- [ ] Green → commit `feat(intel): deterministic completion ranking with a bounded top-N`.

## Task 6: `SemanticEngine::complete`

- [ ] `pub fn complete(&self, req: CompletionRequest<'_>) -> Vec<CompletionItem>`: analyze (cached by revision) → classify → generate → rank → return; `replace` is the current word's range.
- [ ] Tests: one request per §5.2 context against a synthetic catalog, asserting the head of the ranked list; completion never emits diagnostics and never mutates the buffer; stale analysis after an edit reflects the new text (revision path).
- [ ] Green → commit `feat(intel): SemanticEngine::complete over the analysis and catalog`.

## Task 7: Latency probe + living docs

- [ ] `probe.rs`: new ignored release leg on the existing 10 000-object catalog — warm completion requests at relation, expression and qualifier cursors, 200 runs each, report p50/p95/p99/max, assert result sanity only.
- [ ] `docs/12-sql-intelligence.md`: shipped `SemanticEngine::complete`/`CompletionRequest`/`CompletionItem` shapes replace the sketches (including the `&mut Buffer` and `replace` deviations); note the text-lexer classification and why.
- [ ] `docs/14-project-layout.md`: `tempr_intel` row gains completion; `tempr_domain` row gains the shared text helper.
- [ ] `docs/TODO.md`: star expansion stays parked for hover (stage 6); quoted-identifier completion nuance; FROM-subquery correlation tightening now that completion consumes scopes.
- [ ] `docs/PROGRESS.md`: status block; check the two Phase 3 completion boxes only after the probe numbers exist; decisions-log row for the shared matcher + classifier; session-log row with probe numbers.
- [ ] Commit `docs: stage 5 completion living-docs updates`.

## Verification (stage exit)

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo test --release -p tempr_intel -- --ignored --nocapture   # catalog + completion probes
```

PR on `feat/ph3-completion`; review; user merges.
