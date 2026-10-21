# Phase 3 Stage 3 — `tempr_intel` Catalog Cache Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The in-memory half of the catalog: a new `tempr_intel` crate whose `CatalogCache` turns a `SchemaSnapshot` into a resolution-shaped, immutable arena with O(1) identity/id lookups, folded ± exact relation lookups, contiguous per-relation column slices, prefix search, and a release probe that reports build and prefix-search latency on a 10 000-object catalog plus a full `.tcat` → cache load.

**Architecture:** `SchemaSnapshot`'s flat `Vec<SchemaObject>` stays the wire and disk shape; `CatalogCache` is derived data rebuilt wholesale per snapshot. One arena `Vec<CatalogEntry>` holds every object (relations, then functions, then columns grouped contiguously per relation, then indexes); all maps hold `u32` indices into it. The structure is immutable once built — a refresh builds a new one and swaps the `Arc` (the swap lives in stage 4's `SemanticEngine`; stage 3 delivers only the immutable structure). Unquoted SQL identifiers fold to lower case (PostgreSQL's rule); quoted identifiers use the case-sensitive map; both maps point into the same arena. No new dependencies — std types only (spec §4.1).

**Tech Stack:** Rust 1.97.1, existing `tempr_domain` (+ `tempr_workspace` as a dev-dependency for the `.tcat` load probe).

**Spec:** [docs/superpowers/specs/2026-09-07-phase3-sql-intelligence-design.md](../specs/2026-09-07-phase3-sql-intelligence-design.md) — §4.1 (in-memory form), §8 (unit tests + load probe), §9 stage 3.

## Global Constraints

- **Branch, PR, review — never commit to main.** Branch for this stage: `feat/ph3-intel-catalog-cache`. See CLAUDE.md hard rules.
- **Every task ends green**: `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`.
- **No new dependencies.** Std types only. `tempr_workspace` may appear only as a dev-dependency of `tempr_intel` (for the `.tcat` probe); it is an internal crate already in the graph, not a third-party dependency.
- **No unwrap/expect in library code**; tests may use them (`#![cfg_attr(test, allow(...))]`, per `tempr_editor`).
- **`tempr_editor`'s `perf_large_batch_edit_is_linear` fails in debug builds** — a release-only timing probe, unrelated. Ignore it.
- **Identity stays `(kind, native_id)`-derived `SchemaObjectId`** — the cache keys on it verbatim; it must not re-derive or mint ids.
- **Columns are laid out contiguously per relation** in `ordinal` order, so "columns of this table" is a slice, not a scan. A column whose parent relation is not in the cache is dropped (stage 2 already does this at the service, but the cache must not rely on it).
- **Canonical names, do not rename**: `tempr_intel`, `CatalogCache`, `CatalogEntry`, `CatalogStats`, `from_snapshot`.
- The probe is an `#[ignore = "release-only timing probe"]`d test, run with `cargo test --release -p tempr_intel -- --ignored`, mirroring `tempr_editor`'s D21/D22 probes.

---

## Execution notes (2026-10-21, closing the stage)

Tasks 1-3 executed in one feat commit: the crate is a single compile unit, so every task's failing-compile step was shared; tests per task were still written before their implementation step and each task's verification ran green before the next. Task 4's probe deviates from its step list only in the synthetic size (1 200 relations × 8 columns + 1 200 indexes + 800 functions ≈ 12 800 objects instead of the sketch's 9 500) and in searching prefix `rel` (no name starts with `u` in the synthetic set, so that early assertion would have been vacuous). The column lookups are the symmetric `column_exact`/`column_folded` pair instead of the single `column_by_folded_name`̀. Security note: `cargo deny` flagged the rustls/webpki vulnerabilities mid-stage; the patch bumps + zbus-trio ignores land in the same PR (`fix(security)` commit).

---

## File Structure

| File | Responsibility in this stage |
|---|---|
| `crates/tempr_intel/` | **New crate.** `lib.rs` (module wiring + re-exports), `catalog.rs` (`CatalogCache`, `CatalogEntry`, `CatalogStats`), `probe.rs` (release probes, ignored) |
| `Cargo.toml` | workspace member + `tempr_intel.workspace = true` |
| `docs/12-sql-intelligence.md` | Reconcile: `CatalogCache` in `tempr_intel`; public API matches what ships (§11 impact) |
| `docs/14-project-layout.md` | `tempr_intel` now exists (dependency diagram already lists it) |
| `docs/PROGRESS.md`, `docs/TODO.md`, `docs/PRODUCT.md` | Living docs per CLAUDE.md |

Out of scope for this stage: the scan/`SemanticEngine` holder (`RwLock<Arc<...>>` swap), ranking, completion, diagnostics, incremental delta invalidation (TODO row from stage 2 stays parked).

---

## Task 1: Crate scaffold + `CatalogCache::from_snapshot` (arena by_id)

**Files:**
- Create: `crates/tempr_intel/Cargo.toml`, `crates/tempr_intel/src/lib.rs`, `crates/tempr_intel/src/catalog.rs`
- Modify: root `Cargo.toml` (member + workspace dep)
- Test: `crates/tempr_intel/src/catalog.rs` `mod tests`

**Interfaces:**
- Consumes: `tempr_domain::{SchemaSnapshot, SchemaObject, SchemaObjectId, ConnectionId}`
- Produces:
  ```rust
  pub struct CatalogCache { /* arena + maps, private */ }
  pub enum CatalogEntry { Table{..}, View{..}, Column{..}, Index{..}, Function{..} } // owned, borrowed观众 accessors
  pub struct CatalogStats { relations, columns, indexes, functions, schemas, keywords: usize, snapshot_id }
  impl CatalogCache {
      pub fn from_snapshot(snapshot: &SchemaSnapshot) -> Self;
      pub fn snapshot_id(&self) -> SchemaSnapshotId;  pub fn version(&self) -> u64;
      pub fn connection_id(&self) -> ConnectionId;    pub fn len(&self) -> usize;
      pub fn keywords(&self) -> &[String];            pub fn schemas(&self) -> &[Box<str>];
      pub fn stats(&self) -> &CatalogStats;
      pub fn object(&self, idx: u32) -> Option<&CatalogEntry>;
      pub fn by_id(&self, id: SchemaObjectId) -> Option<u32>;
  }
  ```
  Everything later consumes exactly these names.

- [ ] Write failing tests: construction from a snapshot with every object kind; `by_id` resolves each arena index and round-trips `object(by_id(id)).id`; an unknown id is `None`; arena grows only by objects present; `keywords`/`schemas`/`stats` populated.
- [ ] `cargo test -p tempr_intel` → compile error.
- [ ] Scaffold the crate (mirrors `tempr_editor`: `#![deny(unsafe_code)]`, `cfg_attr(test, allow(clippy::expect_used, clippy::unwrap_used))`, workspace lints).
- [ ] Implement arena + `from_snapshot` (three grouped passes: relations+functions first to resolve parent indices, then columns grouped per relation in `ordinal` order, then indexes). Duplicates of the same `SchemaObjectId` in the snapshot: last one wins (an incremental splice never produces them; a defensive rule only).
- [ ] Green → run `cargo test --workspace`, fmt, clippy → Commit `feat(intel): tempr_intel crate with immutable CatalogCache arena`.

---

## Task 2: Lookups — relations (exact/folded), columns, schemas

**Files:**
- Modify: `crates/tempr_intel/src/catalog.rs`
- Test: same file

**Interfaces:**
- Produces:
  ```rust
  impl CatalogCache {
      pub fn relation_exact(&self, schema: &str, name: &str) -> Option<u32>;   // quotedIdentifier path
      pub fn relation_folded(&self, schema: &str, name: &str) -> Vec<u32>;     // unquoted path; >1 = ambiguous
      pub fn columns_of(&self, rel_idx: u32) -> &[CatalogEntry];               // contiguous slice, ordinal order
      pub fn column_by_folded_name(&self, rel_idx: u32, name: &str) -> Option<u32>; // folded + quoted-exact column look
      pub fn relation_count(&self) -> usize; // relations arena prefix length ([0..relations])
  }
  ```
  Fold rule: full Unicode fold to lower case (`to_lowercase`), matching PostgreSQL's unquoted-identifier semantics as closely as full-folding goes.

- [ ] Failing tests: `"Users"` quoted vs `users` unquoted in the same schema — exact map finds each, folded returns both (ambiguity is a `Vec`, the analyzer decides); unknown schema/name empty; case-folding (`SELECT * from PUBLIC.USERS` style lookup hits `public.users`); columns contiguous and ordinal-ordered; column lookup folded; childless relation → empty slice; a column whose parent id is missing is dropped, not 一 panic.
- [ ] Implement. `columns_of` indexes a `HashMap<u32, Range<u32>>` keyed by relation arena index.
- [ ] Green → fmt/clippy/tests → Commit `feat(intel): relation, schema and column lookups over the catalog arena`.

---

## Task 3: Prefix search

**Files:**
- Modify: `crates/tempr_intel/src/catalog.rs`
- Test: same file

**Interfaces:**
- Produces:
  ```rust
  impl CatalogCache {
      /// Case-folded prefix search over relation + function names, sorted by
      /// (folded_name, idx) for deterministic output. `cap` bounds the scan;
      /// the boolean says whether more candidates exist beyond the cap.
      pub fn prefix_search(&self, prefix: &str, cap: usize) -> (Vec<u32>, bool);
  }
  ```
  `name_order: Vec<(Box<str>, u32)>` — folded name, arena idx, sorted; binary search (`partition_point`) + scan of the matching run. Empty prefix matches everything (bounded by `cap`).

- [ ] Failing tests: prefix hits mid-array and array-end boundaries; no match; fold case-insensitivity; determinism (two caches built from the same snapshot produce the same order); cap respected with the `more` flag true; empty catalog empty.
- [ ] Implement → green → fmt/clippy/tests → Commit `feat(intel): folded prefix search over relation and function names`.

---

## Task 4: Load probe (ignored, release)

**Files:**
- Create: `crates/tempr_intel/src/probe.rs`; add `mod probe` to `lib.rs`
- Modify: `crates/tempr_intel/Cargo.toml` (`[dev-dependencies] tempr_workspace.workspace = true`)

**Interfaces:**
- Consumes: the synthetic catalog builder; `tempr_workspace::{encode_catalog, decode_catalog}` for the on-disk leg.

- [ ] Write the probe: build a synthetic snapshot with **10 000 objects** (1 000 relations × 8 columns + 1 000 indexes + 500 functions + keywords) deterministically; measure, over ≥ 20 runs with p50/p95/p99/max:
  1. `CatalogCache::from_snapshot` build;
  2. folded prefix search + `columns_of` + `relation_folded` hot-path mix;
  3. `.tcat` load — `encode_catalog` once, then per run `decode_catalog` + `from_snapshot` (in-memory bytes; no disk I/O so the number isolates codec + build, the OD#1 concern).
  `#[ignore = "release-only timing probe"]`, asserts nothing about timing, only sanity (candidate counts). Mirrors `tempr_editor`'s D21/D22 probes.
- [ ] Run release: `cargo test --release -p tempr_intel -- --ignored`; record the numbers in PROGRESS.md.
- [ ] fmt/clippy/tests → Commit `perf(intel): load probe — build, hot-path mix and .tcat round-trip on a 10k-object catalog`.

---

## Task 5: Living docs

- [ ] `docs/12-sql-intelligence.md`: the catalog-cache section points at `tempr_intel::CatalogCache` with the shipped API; sketched API reconciled (engine's field shape changes arrive stage 4).
- [ ] `docs/14-project-layout.md`: `tempr_intel` exists; crate table row updated with what it currently holds.
- [ ] `docs/PROGRESS.md`: status block, Phase 3 checklist unchanged (no new box completes — stage 3 is en route to the completion/latency boxes), decisions log row, session log row with probe numbers.
- [ ] `docs/PRODUCT.md`: section 4 markers — no user-visible feature yet, so nothing changes unless the doc claims one exists.
- [ ] `docs/TODO.md`: nothing newly deferred beyond the parked delta row; confirm.
- [ ] Commit `docs: stage 3 living-docs updates` (or fold into the final commit).

---

## Verification (stage exit)

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo test --release -p tempr_intel -- --ignored   # probe numbers
cargo deny check
```

PR: `gh pr create` on `feat/ph3-intel-catalog-cache`; review; user merges.
