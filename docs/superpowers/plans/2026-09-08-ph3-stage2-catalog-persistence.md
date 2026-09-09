# Phase 3 Stage 2 — Catalog Persistence Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Turn stage 1's driver capabilities into a persisted, diffable catalog: stable object identity, a `.tcat` cache file per connection, and a `SchemaService` that refreshes fully or incrementally and survives a restart offline.

**Architecture:** `SchemaObjectId` stops being random and becomes a deterministic UUIDv5 over `(connection, kind, native_id)`. `SchemaSnapshot` grows the two things a cache needs to be diffable — the server's keyword list and the fingerprints the snapshot was built from. A new `catalog` module in `tempr_workspace` encodes a snapshot behind a versioned header and writes it atomically through `Storage`. `SchemaService` gains a full refresh (search-path scoped, identity-derived, cache-writing) and an incremental refresh that sweeps fingerprints, re-introspects only affected schemas, and rewrites the cache only when content actually changed.

**Tech Stack:** Rust 1.97.1, `serde` + `bincode` (new, see Task 3), `uuid` v5 feature, `tokio`, existing `tempr_domain` / `tempr_workspace` / `tempr_services` crates.

**Spec:** [docs/superpowers/specs/2026-09-07-phase3-sql-intelligence-design.md](../specs/2026-09-07-phase3-sql-intelligence-design.md) — §3 (identity), §4.2 (on-disk form), §4.3 (refresh), §4.4 (scope), §4.5 (keywords), §9 stage 2.

## Global Constraints

- **Branch, PR, review — never commit to main.** Branch for this stage: `feat/ph3-catalog-persistence`. See CLAUDE.md hard rules.
- **Every task ends green**: `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`.
- **Integration tests need a live PostgreSQL**:
  ```bash
  docker start tempr-pg && export DATABASE_URL='postgres://tempr:tempr@localhost:55432/tempr?sslmode=disable'
  ```
  Run with `cargo test --workspace -- --ignored`. Live-server tests carry `#[ignore = "requires DATABASE_URL env var pointing to a live PostgreSQL instance"]`.
- **`tempr_editor`'s `perf_large_batch_edit_is_linear` fails in debug builds.** It is a release-only timing probe, not a regression. Ignore it.
- **One new dependency, and only one**: `bincode`. It gets a DECISIONS.md entry in Task 3 and must pass `cargo deny check`. No other new crate — `uuid`'s `v5` is a feature of a crate already in the graph, not a new dependency.
- **Identity is keyed on `(kind, native_id)`, never `native_id` alone.** Stage 1 documented this on `SchemaSnapshotEntry` and `SchemaFingerprint`: column ids are packed `(attrelid << 16) | attnum` and can collide with a raw `pg_class.oid` on a database whose OID counter has wrapped.
- **The cache is derived data.** Any unreadable, truncated, wrong-version or hash-mismatched file is discarded and re-introspected. A cache failure never surfaces as a user-facing error, only a log line.
- **No unwrap/expect in library code**; tests may use them.
- **SQL values are bound as `$N`**, never interpolated.
- **Canonical names, do not rename**: `SchemaObjectKind`, `SchemaObjectId::derived`, `CatalogCacheFile`, `Storage::catalog_cache`, `SchemaService::refresh_incremental`, `CATALOG_FORMAT_VERSION`.
- Conventional commits, each ending with:
  `Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>`

---

## File Structure

| File | Responsibility in this stage |
|---|---|
| `crates/tempr_domain/src/ids.rs` | `SchemaObjectId::derived(connection, kind, native_id)` — deterministic UUIDv5 |
| `crates/tempr_domain/src/schema.rs` | `SchemaObjectKind`; `SchemaSnapshot` gains `keywords` and `fingerprints`; `SchemaFingerprintRecord` |
| `crates/tempr_workspace/src/catalog.rs` | **New.** `.tcat` encode/decode behind a versioned header, content hash, `CatalogCacheFile` |
| `crates/tempr_workspace/src/storage.rs` | `Storage::catalog_cache(connection)` + `FileSystemStorage` implementation |
| `crates/tempr_workspace/src/lib.rs` | Re-export the catalog types |
| `crates/tempr_services/src/schema.rs` | Search-path scope, derived ids, keywords, fingerprints, cache load/save, `refresh_incremental` |
| `crates/tempr/tests/integration.rs` | Live-server tests: identity stability, incremental delta, offline reload |
| `Cargo.toml`, `crates/*/Cargo.toml` | `bincode` dependency; `uuid` `v5` feature |
| `docs/DECISIONS.md` | D25 (identity), D26 (fingerprint refresh), D27 (cache format + bincode) |
| `docs/07-storage.md`, `docs/05-services.md`, `docs/PROGRESS.md`, `docs/TODO.md` | OD#1 resolved, service contract, living docs |

---

## Task 1: Deterministic object identity

**Files:**
- Modify: `crates/tempr_domain/src/ids.rs`
- Modify: `crates/tempr_domain/src/schema.rs` (add `SchemaObjectKind`)
- Modify: `Cargo.toml` (uuid `v5` feature)
- Test: same files, `#[cfg(test)] mod tests`

**Interfaces:**
- Consumes: nothing
- Produces:
  ```rust
  pub enum SchemaObjectKind { Table, View, Column, Index, Function }
  impl SchemaObjectId { pub fn derived(connection: ConnectionId, kind: SchemaObjectKind, native_id: u64) -> Self; }
  impl SchemaObject { pub fn kind(&self) -> SchemaObjectKind; }
  ```
  Every later task derives ids through this and nothing else.

- [ ] **Step 1: Write the failing tests**

In `crates/tempr_domain/src/ids.rs`, inside `mod tests`:

```rust
    #[test]
    fn derived_ids_are_stable_and_kind_separated() {
        use crate::schema::SchemaObjectKind;
        let conn = ConnectionId::new();
        let other = ConnectionId::new();

        // Same inputs → same id, every time, in any process.
        assert_eq!(
            SchemaObjectId::derived(conn, SchemaObjectKind::Table, 16384),
            SchemaObjectId::derived(conn, SchemaObjectKind::Table, 16384)
        );
        // Kind is part of the key: a column id may numerically equal a relation oid.
        assert_ne!(
            SchemaObjectId::derived(conn, SchemaObjectKind::Table, 16384),
            SchemaObjectId::derived(conn, SchemaObjectKind::Column, 16384)
        );
        // Connection is part of the key: two databases may share oids.
        assert_ne!(
            SchemaObjectId::derived(conn, SchemaObjectKind::Table, 16384),
            SchemaObjectId::derived(other, SchemaObjectKind::Table, 16384)
        );
        // Different objects of the same kind stay distinct.
        assert_ne!(
            SchemaObjectId::derived(conn, SchemaObjectKind::Table, 16384),
            SchemaObjectId::derived(conn, SchemaObjectKind::Table, 16385)
        );
    }

    #[test]
    fn derived_id_is_reproducible_across_runs() {
        use crate::schema::SchemaObjectKind;
        use uuid::Uuid;
        // A fixed connection id pins the expected output, so a change to the
        // derivation scheme fails here instead of silently orphaning caches.
        let conn = ConnectionId(Uuid::from_u128(0x0123_4567_89ab_cdef_0123_4567_89ab_cdef));
        let id = SchemaObjectId::derived(conn, SchemaObjectKind::Column, (16384u64 << 16) | 2);
        assert_eq!(id, SchemaObjectId::derived(conn, SchemaObjectKind::Column, (16384u64 << 16) | 2));
        assert_ne!(id.0, Uuid::nil());
    }
```

In `crates/tempr_domain/src/schema.rs`, inside `mod tests`:

```rust
    #[test]
    fn schema_object_reports_its_kind() {
        let obj = SchemaObject::Column {
            id: SchemaObjectId::new(),
            parent_id: SchemaObjectId::new(),
            name: "id".to_string(),
            data_type: "int8".to_string(),
            nullable: false,
            ordinal: 1,
            default: None,
        };
        assert_eq!(obj.kind(), SchemaObjectKind::Column);
    }
```

- [ ] **Step 2: Run them and watch them fail**

```bash
cargo test -p tempr_domain
```

Expected: compile errors — `SchemaObjectKind` and `derived` do not exist.

- [ ] **Step 3: Enable the uuid v5 feature**

In the root `Cargo.toml`, workspace dependencies:

```toml
uuid        = { version = "1", features = ["v4", "v5", "serde"] }
```

This is a feature of a crate already in the graph, not a new dependency.

- [ ] **Step 4: Add the kind enum and the accessor**

In `crates/tempr_domain/src/schema.rs`, above `SchemaSnapshot`:

```rust
/// What a schema object is. Part of every object's identity: `native_id` is
/// unique only within a kind (a packed column id can numerically equal a
/// relation OID), so consumers key on the pair.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SchemaObjectKind {
    Table,
    View,
    Column,
    Index,
    Function,
}

impl SchemaObjectKind {
    /// Stable discriminant used in identity derivation. Never renumber these:
    /// a change orphans every cache file in the wild.
    pub fn discriminant(self) -> u8 {
        match self {
            SchemaObjectKind::Table => 0,
            SchemaObjectKind::View => 1,
            SchemaObjectKind::Column => 2,
            SchemaObjectKind::Index => 3,
            SchemaObjectKind::Function => 4,
        }
    }
}
```

And on `impl SchemaObject`, beside the existing `id()`:

```rust
    pub fn kind(&self) -> SchemaObjectKind {
        match self {
            SchemaObject::Table { .. } => SchemaObjectKind::Table,
            SchemaObject::View { .. } => SchemaObjectKind::View,
            SchemaObject::Column { .. } => SchemaObjectKind::Column,
            SchemaObject::Index { .. } => SchemaObjectKind::Index,
            SchemaObject::Function { .. } => SchemaObjectKind::Function,
        }
    }
```

- [ ] **Step 5: Add the derivation**

In `crates/tempr_domain/src/ids.rs`, after the `uuid_id!` invocations:

```rust
impl SchemaObjectId {
    /// Identity derived from the database engine's own identifier, stable
    /// across refreshes, restarts and renames.
    ///
    /// UUIDv5 over the connection id as namespace and `(kind, native_id)` as
    /// name, so the same object in the same database always resolves to the
    /// same id — which is what lets a cached snapshot be diffed against a
    /// fresh one. `kind` is part of the key because `native_id` is unique
    /// only within a kind.
    pub fn derived(
        connection: crate::ids::ConnectionId,
        kind: crate::schema::SchemaObjectKind,
        native_id: u64,
    ) -> Self {
        let mut name = [0u8; 9];
        name[0] = kind.discriminant();
        name[1..].copy_from_slice(&native_id.to_le_bytes());
        Self(Uuid::new_v5(&connection.0, &name))
    }
}
```

- [ ] **Step 6: Run the tests**

```bash
cargo test -p tempr_domain
```

Expected: PASS, including the two new tests.

- [ ] **Step 7: Commit**

```bash
git add Cargo.toml Cargo.lock crates/tempr_domain/src/ids.rs crates/tempr_domain/src/schema.rs
git commit -m "feat(domain): deterministic SchemaObjectId derived from (connection, kind, native_id)"
```

---

## Task 2: Snapshot carries keywords and fingerprints

**Files:**
- Modify: `crates/tempr_domain/src/schema.rs`
- Test: same file

**Interfaces:**
- Consumes: `SchemaObjectKind` from Task 1
- Produces:
  ```rust
  pub struct SchemaFingerprintRecord { pub kind: SchemaObjectKind, pub native_id: u64, pub version: u64 }
  // SchemaSnapshot gains: pub keywords: Vec<String>, pub fingerprints: Vec<SchemaFingerprintRecord>
  ```
  Task 5 diffs `fingerprints` against a fresh sweep; the cache file in Task 3 persists both.

- [ ] **Step 1: Write the failing test**

In `crates/tempr_domain/src/schema.rs`, inside `mod tests`:

```rust
    #[test]
    fn snapshot_serde_carries_keywords_and_fingerprints() {
        let mut snapshot = make_snapshot(vec![]);
        snapshot.keywords = vec!["select".to_string(), "join".to_string()];
        snapshot.fingerprints = vec![SchemaFingerprintRecord {
            kind: SchemaObjectKind::Table,
            native_id: 16384,
            version: 42,
        }];

        let json = serde_json::to_string(&snapshot).expect("serialize");
        let back: SchemaSnapshot = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.keywords, snapshot.keywords);
        assert_eq!(back.fingerprints.len(), 1);
        assert_eq!(back.fingerprints[0].native_id, 16384);
        assert_eq!(back.fingerprints[0].kind, SchemaObjectKind::Table);
    }

    #[test]
    fn older_snapshots_without_the_new_fields_still_load() {
        // A snapshot serialized before these fields existed must not fail to
        // parse — the cache would otherwise be discarded on every upgrade.
        let json = r#"{
            "id": "00000000-0000-0000-0000-000000000001",
            "connection_id": "00000000-0000-0000-0000-000000000002",
            "version": 3,
            "fetched_at": "2026-09-08T00:00:00Z",
            "objects": []
        }"#;
        let back: SchemaSnapshot = serde_json::from_str(json).expect("deserialize");
        assert!(back.keywords.is_empty());
        assert!(back.fingerprints.is_empty());
    }
```

- [ ] **Step 2: Run it and watch it fail**

```bash
cargo test -p tempr_domain snapshot_serde_carries_keywords_and_fingerprints
```

Expected: compile error — no field `keywords` on `SchemaSnapshot`.

- [ ] **Step 3: Add the record type and the fields**

In `crates/tempr_domain/src/schema.rs`:

```rust
/// One object's change marker, as reported by the driver's fingerprint sweep
/// and persisted with the snapshot so a later sweep can be diffed against it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SchemaFingerprintRecord {
    pub kind: SchemaObjectKind,
    pub native_id: u64,
    pub version: u64,
}
```

And on `SchemaSnapshot`, after `objects`:

```rust
    /// The engine's keyword list, fetched with the snapshot so completion
    /// never queries the database (docs/12-sql-intelligence.md).
    #[serde(default)]
    pub keywords: Vec<String>,
    /// Change markers for every object in scope at the time of the snapshot.
    /// An incremental refresh diffs a fresh sweep against these.
    #[serde(default)]
    pub fingerprints: Vec<SchemaFingerprintRecord>,
```

`#[serde(default)]` on both keeps older cache files loadable.

- [ ] **Step 4: Fix every construction site**

`SchemaSnapshot` is built in `crates/tempr_services/src/schema.rs` and in domain tests. Add `keywords: Vec::new(), fingerprints: Vec::new()` to each; the service fills them for real in Task 5.

- [ ] **Step 5: Run the tests**

```bash
cargo test --workspace
```

Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/tempr_domain/src/schema.rs crates/tempr_services/src/schema.rs
git commit -m "feat(domain): snapshots carry the keyword list and their fingerprints"
```

---

## Task 3: The `.tcat` codec

**Files:**
- Create: `crates/tempr_workspace/src/catalog.rs`
- Modify: `crates/tempr_workspace/src/lib.rs`, `crates/tempr_workspace/Cargo.toml`, root `Cargo.toml`
- Create: `docs/DECISIONS.md` entry D27
- Test: `crates/tempr_workspace/src/catalog.rs` `mod tests`

**Interfaces:**
- Consumes: `SchemaSnapshot` from Task 2
- Produces:
  ```rust
  pub const CATALOG_MAGIC: [u8; 4] = *b"TCAT";
  pub const CATALOG_FORMAT_VERSION: u16 = 1;
  pub fn encode_catalog(snapshot: &SchemaSnapshot) -> Result<Vec<u8>, WorkspaceError>;
  pub fn decode_catalog(bytes: &[u8]) -> Result<Option<SchemaSnapshot>, WorkspaceError>; // None = discard
  pub fn content_hash(body: &[u8]) -> u64;
  ```
  Task 4 writes and reads these bytes through `Storage`.

- [ ] **Step 1: Write the failing tests**

Create `crates/tempr_workspace/src/catalog.rs` with only its `mod tests` block for now:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use tempr_domain::{ConnectionId, SchemaObject, SchemaObjectId, SchemaSnapshotId};

    fn sample() -> SchemaSnapshot {
        SchemaSnapshot {
            id: SchemaSnapshotId::new(),
            connection_id: ConnectionId::new(),
            version: 7,
            fetched_at: chrono::Utc::now(),
            objects: vec![SchemaObject::Table {
                id: SchemaObjectId::new(),
                schema: "public".to_string(),
                name: "users".to_string(),
                estimated_rows: Some(42),
            }],
            keywords: vec!["select".to_string()],
            fingerprints: vec![],
        }
    }

    #[test]
    fn roundtrip_preserves_the_snapshot() {
        let snapshot = sample();
        let bytes = encode_catalog(&snapshot).expect("encode");
        let back = decode_catalog(&bytes).expect("decode").expect("not discarded");
        assert_eq!(back.id, snapshot.id);
        assert_eq!(back.version, 7);
        assert_eq!(back.objects.len(), 1);
        assert_eq!(back.keywords, vec!["select".to_string()]);
    }

    #[test]
    fn foreign_magic_is_discarded_not_errored() {
        let mut bytes = encode_catalog(&sample()).expect("encode");
        bytes[0] = b'X';
        assert!(decode_catalog(&bytes).expect("no error").is_none());
    }

    #[test]
    fn newer_format_version_is_discarded() {
        let mut bytes = encode_catalog(&sample()).expect("encode");
        let next = CATALOG_FORMAT_VERSION + 1;
        bytes[4..6].copy_from_slice(&next.to_le_bytes());
        assert!(decode_catalog(&bytes).expect("no error").is_none());
    }

    #[test]
    fn truncated_and_corrupt_bodies_are_discarded() {
        let bytes = encode_catalog(&sample()).expect("encode");
        assert!(decode_catalog(&bytes[..bytes.len() / 2]).expect("no error").is_none());
        assert!(decode_catalog(&[]).expect("no error").is_none());

        // A body edited without updating the header hash must not be trusted.
        let mut tampered = bytes.clone();
        let last = tampered.len() - 1;
        tampered[last] ^= 0xff;
        assert!(decode_catalog(&tampered).expect("no error").is_none());
    }

    #[test]
    fn identical_snapshots_hash_identically() {
        let snapshot = sample();
        let a = encode_catalog(&snapshot).expect("encode");
        let b = encode_catalog(&snapshot).expect("encode");
        assert_eq!(a, b, "encoding must be deterministic for change detection");
    }
}
```

- [ ] **Step 2: Run and watch it fail**

```bash
cargo test -p tempr_workspace catalog
```

Expected: the module does not compile — nothing is defined yet.

- [ ] **Step 3: Add the dependency and record the decision**

Root `Cargo.toml`, under "Data / serialisation":

```toml
bincode     = { version = "2", features = ["serde"] }
```

`crates/tempr_workspace/Cargo.toml`:

```toml
bincode.workspace = true
chrono.workspace  = true
```

Then add a DECISIONS.md entry — index row and body — as **D27**:

> **D27 — Catalog cache format is bincode behind a versioned header (2026-09-08)**
>
> **By**: Claude (Phase 3 stage 2), implementing spec decision 4.
> **Decision**: `.tcat` files are a fixed header (magic `TCAT`, `u16` format version, `u16` flags, `u64` content hash) followed by `bincode` of the `SchemaSnapshot`. `bincode` 2.x is adopted as a dependency under the D18 rule (small, pure-Rust, already-serde-shaped). Any file whose magic, version or hash does not match is discarded and re-introspected.
> **Why**: the catalog is derived data, so the cheapest safe failure mode is to throw it away; that makes format evolution a version bump rather than a migration. `bincode` needs no schema and reuses the serde derives the domain already has. Resolves OD#1 in 07-storage, which had weighed `rkyv` and an SQLite table — `rkyv`'s zero-copy win is real but unmeasured, and it buys a `SAFETY` burden before any number justifies it.
> **Consequences**: a second serialization format in the tree (TOML for manifests, JSON for storage, bincode for caches). `CATALOG_FORMAT_VERSION` must be bumped on any layout change, and the load probe in the spec's §8 is the evidence that would justify revisiting the choice.

- [ ] **Step 4: Implement the codec**

At the top of `crates/tempr_workspace/src/catalog.rs`, above the tests:

```rust
//! The on-disk catalog cache (`.tcat`): a versioned header plus a bincode
//! body (docs/07-storage.md → catalog cache, D27). The cache is derived data,
//! so every decode failure is a discard, never an error the user sees.

use tempr_domain::SchemaSnapshot;

use crate::error::WorkspaceError;

pub const CATALOG_MAGIC: [u8; 4] = *b"TCAT";
/// Bump on ANY layout change: older readers discard what they cannot parse.
pub const CATALOG_FORMAT_VERSION: u16 = 1;
const HEADER_LEN: usize = 4 + 2 + 2 + 8;

/// FNV-1a over the body. Not cryptographic — this detects truncation and bit
/// rot, and tells a writer whether the content actually changed.
pub fn content_hash(body: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in body {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x1000_0000_01b3);
    }
    hash
}

/// Encode a snapshot into header + body.
pub fn encode_catalog(snapshot: &SchemaSnapshot) -> Result<Vec<u8>, WorkspaceError> {
    let body = bincode::serde::encode_to_vec(snapshot, bincode::config::standard()).map_err(|e| {
        WorkspaceError::Corrupted {
            reason: format!("catalog encode failed: {e}"),
        }
    })?;
    let mut out = Vec::with_capacity(HEADER_LEN + body.len());
    out.extend_from_slice(&CATALOG_MAGIC);
    out.extend_from_slice(&CATALOG_FORMAT_VERSION.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes()); // flags, reserved
    out.extend_from_slice(&content_hash(&body).to_le_bytes());
    out.extend_from_slice(&body);
    Ok(out)
}

/// Decode a `.tcat` file. `Ok(None)` means "not usable, re-introspect" —
/// foreign magic, a format this build does not know, a truncated file, or a
/// body that does not match its header hash. `Err` is reserved for conditions
/// the caller genuinely cannot proceed past.
pub fn decode_catalog(bytes: &[u8]) -> Result<Option<SchemaSnapshot>, WorkspaceError> {
    if bytes.len() < HEADER_LEN || bytes[..4] != CATALOG_MAGIC {
        return Ok(None);
    }
    let version = u16::from_le_bytes([bytes[4], bytes[5]]);
    if version != CATALOG_FORMAT_VERSION {
        return Ok(None);
    }
    let mut hash_bytes = [0u8; 8];
    hash_bytes.copy_from_slice(&bytes[8..16]);
    let expected = u64::from_le_bytes(hash_bytes);
    let body = &bytes[HEADER_LEN..];
    if content_hash(body) != expected {
        return Ok(None);
    }
    match bincode::serde::decode_from_slice::<SchemaSnapshot, _>(body, bincode::config::standard())
    {
        Ok((snapshot, _)) => Ok(Some(snapshot)),
        Err(_) => Ok(None),
    }
}
```

Register the module in `crates/tempr_workspace/src/lib.rs`:

```rust
pub mod catalog;
```

and re-export:

```rust
pub use catalog::{CATALOG_FORMAT_VERSION, CATALOG_MAGIC, content_hash, decode_catalog, encode_catalog};
```

- [ ] **Step 5: Run the tests, then the dependency audit**

```bash
cargo test -p tempr_workspace
cargo deny check
```

Expected: tests PASS; `cargo deny check` reports advisories/bans/licenses/sources ok. If `bincode`'s license is not already on the allowlist, add it in `deny.toml` and say so in your report.

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml Cargo.lock deny.toml crates/tempr_workspace/ docs/DECISIONS.md
git commit -m "feat(workspace): .tcat catalog cache codec behind a versioned header"
```

---

## Task 4: `Storage::catalog_cache`

**Files:**
- Modify: `crates/tempr_workspace/src/storage.rs`, `crates/tempr_workspace/src/lib.rs`
- Test: `crates/tempr_workspace/src/storage.rs` `mod tests`

**Interfaces:**
- Consumes: `encode_catalog` / `decode_catalog` from Task 3
- Produces:
  ```rust
  #[async_trait]
  pub trait CatalogCacheFile: Send + Sync {
      async fn load(&self) -> Result<Option<SchemaSnapshot>, WorkspaceError>;
      async fn save(&self, snapshot: &SchemaSnapshot) -> Result<(), WorkspaceError>;
      fn path(&self) -> PathBuf;
  }
  // on Storage:
  fn catalog_cache(&self, connection: ConnectionId) -> Box<dyn CatalogCacheFile>;
  ```
  Task 5 calls exactly these.

- [ ] **Step 1: Write the failing tests**

In `crates/tempr_workspace/src/storage.rs` `mod tests`:

```rust
    fn sample_snapshot(connection: tempr_domain::ConnectionId) -> tempr_domain::SchemaSnapshot {
        tempr_domain::SchemaSnapshot {
            id: tempr_domain::SchemaSnapshotId::new(),
            connection_id: connection,
            version: 1,
            fetched_at: chrono::Utc::now(),
            objects: vec![],
            keywords: vec!["select".to_string()],
            fingerprints: vec![],
        }
    }

    #[tokio::test]
    async fn catalog_cache_roundtrips_and_starts_empty() {
        let (_dir, storage) = make_storage().await;
        let connection = tempr_domain::ConnectionId::new();
        let cache = storage.catalog_cache(connection);

        assert!(cache.load().await.expect("load").is_none(), "no file yet");

        let snapshot = sample_snapshot(connection);
        cache.save(&snapshot).await.expect("save");
        let loaded = cache.load().await.expect("load").expect("file exists");
        assert_eq!(loaded.id, snapshot.id);
        assert_eq!(loaded.keywords, vec!["select".to_string()]);
    }

    #[tokio::test]
    async fn each_connection_gets_its_own_file() {
        let (_dir, storage) = make_storage().await;
        let a = tempr_domain::ConnectionId::new();
        let b = tempr_domain::ConnectionId::new();
        storage.catalog_cache(a).save(&sample_snapshot(a)).await.expect("save a");

        assert!(storage.catalog_cache(b).load().await.expect("load b").is_none());
        assert_ne!(storage.catalog_cache(a).path(), storage.catalog_cache(b).path());
    }

    #[tokio::test]
    async fn a_corrupt_cache_file_loads_as_none() {
        let (_dir, storage) = make_storage().await;
        let connection = tempr_domain::ConnectionId::new();
        let cache = storage.catalog_cache(connection);
        cache.save(&sample_snapshot(connection)).await.expect("save");

        tokio::fs::write(cache.path(), b"not a catalog file at all")
            .await
            .expect("corrupt the file");
        assert!(cache.load().await.expect("no error").is_none());
    }

    #[tokio::test]
    async fn saving_leaves_no_temp_file_behind() {
        let (_dir, storage) = make_storage().await;
        let connection = tempr_domain::ConnectionId::new();
        let cache = storage.catalog_cache(connection);
        cache.save(&sample_snapshot(connection)).await.expect("save");

        let dir = cache.path().parent().expect("parent").to_path_buf();
        let mut entries = tokio::fs::read_dir(&dir).await.expect("read dir");
        let mut names = Vec::new();
        while let Some(e) = entries.next_entry().await.expect("entry") {
            names.push(e.file_name().to_string_lossy().to_string());
        }
        assert!(
            names.iter().all(|n| !n.ends_with(".tmp")),
            "atomic write must not leave a temp file: {names:?}"
        );
    }
```

- [ ] **Step 2: Run and watch it fail**

```bash
cargo test -p tempr_workspace catalog_cache
```

Expected: compile error — no method `catalog_cache` on `Storage`.

- [ ] **Step 3: Implement**

In `crates/tempr_workspace/src/storage.rs`, add to the `Storage` trait (a non-async method returning a handle, so callers can ask for the path without touching the disk):

```rust
    /// Handle to this connection's catalog cache file. Creating the handle
    /// touches no disk; `load` and `save` do.
    fn catalog_cache(&self, connection: ConnectionId) -> Box<dyn CatalogCacheFile>;
```

Then the trait and the file-system implementation:

```rust
/// Read/write access to one connection's `.tcat` cache. Every failure to read
/// is reported as `Ok(None)`: the cache is derived data and is rebuilt rather
/// than repaired.
#[async_trait]
pub trait CatalogCacheFile: Send + Sync {
    async fn load(&self) -> Result<Option<SchemaSnapshot>, WorkspaceError>;
    async fn save(&self, snapshot: &SchemaSnapshot) -> Result<(), WorkspaceError>;
    fn path(&self) -> PathBuf;
}

pub struct FileCatalogCache {
    path: PathBuf,
}

#[async_trait]
impl CatalogCacheFile for FileCatalogCache {
    async fn load(&self) -> Result<Option<SchemaSnapshot>, WorkspaceError> {
        let bytes = match tokio::fs::read(&self.path).await {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => {
                tracing::warn!(error = %e, path = %self.path.display(), "catalog cache unreadable");
                return Ok(None);
            }
        };
        crate::catalog::decode_catalog(&bytes)
    }

    async fn save(&self, snapshot: &SchemaSnapshot) -> Result<(), WorkspaceError> {
        let bytes = crate::catalog::encode_catalog(snapshot)?;
        if let Some(parent) = self.path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        let tmp = self.path.with_extension("tcat.tmp");
        tokio::fs::write(&tmp, &bytes).await?;
        tokio::fs::rename(&tmp, &self.path).await?;
        Ok(())
    }

    fn path(&self) -> PathBuf {
        self.path.clone()
    }
}
```

and on `impl Storage for FileSystemStorage`:

```rust
    fn catalog_cache(&self, connection: ConnectionId) -> Box<dyn CatalogCacheFile> {
        Box::new(FileCatalogCache {
            path: self
                .tempr_dir()
                .join("cache")
                .join("catalog")
                .join(format!("{}.tcat", connection.0)),
        })
    }
```

Add the imports this needs (`tempr_domain::{ConnectionId, SchemaSnapshot}`) and re-export `CatalogCacheFile` and `FileCatalogCache` from `lib.rs`. If `tracing` is not already a dependency of `tempr_workspace`, use the crate's existing logging approach instead of adding one — check before assuming.

- [ ] **Step 4: Run the tests**

```bash
cargo test -p tempr_workspace
```

Expected: PASS, including the four new tests.

- [ ] **Step 5: Commit**

```bash
git add crates/tempr_workspace/
git commit -m "feat(workspace): per-connection catalog cache files through Storage"
```

---

## Task 5: Full refresh with identity, keywords and cache write

**Files:**
- Modify: `crates/tempr_services/src/schema.rs`
- Modify: `crates/tempr_services/Cargo.toml` (add `tempr_workspace.workspace = true`)
- Modify: `crates/tempr/src/main.rs` (construct the service with a cache)
- Test: `crates/tempr_services/src/schema.rs` `mod tests`

**Interfaces:**
- Consumes: Tasks 1-4
- Produces:
  ```rust
  impl SchemaService {
      pub fn with_cache(event_bus: Arc<EventBus>, connection_service: Arc<ConnectionService>,
                        storage: Arc<dyn Storage>) -> Arc<Self>;
      pub async fn load_cached(&self, connection_id: ConnectionId) -> Option<Arc<SchemaSnapshot>>;
      // refresh() keeps its signature; its behaviour changes
  }
  ```
  Task 6 diffs against what `refresh` stores; Task 7 tests it against a live server.

- [ ] **Step 1: Write the failing unit test**

The existing `mod tests` in `crates/tempr_services/src/schema.rs` drives the service with mock connections. Add:

```rust
    #[tokio::test]
    async fn refresh_derives_stable_ids_and_stores_keywords() {
        // The mock connection returns one table with one column, plus a
        // keyword list and matching fingerprints.
        let (bus, cs) = mock_services_with_schema();
        let connection = mock_connection_id();
        let service = SchemaService::new(bus, cs);

        let first = service.refresh(connection).await.expect("first refresh");
        let second = service.refresh(connection).await.expect("second refresh");

        let ids = |snapshot: &SchemaSnapshot| -> Vec<SchemaObjectId> {
            let mut v: Vec<_> = snapshot.objects.iter().map(|o| o.id()).collect();
            v.sort_by_key(|id| id.0);
            v
        };
        assert_eq!(ids(&first), ids(&second), "ids must not change between refreshes");
        assert!(!first.keywords.is_empty(), "keywords come from the driver");
        assert_eq!(
            first.fingerprints.len(),
            first.objects.len(),
            "every object carries a fingerprint the next sweep can diff"
        );
        assert_eq!(second.version, first.version + 1);
    }

    #[tokio::test]
    async fn refresh_passes_through_estimated_rows_and_view_definitions() {
        let (bus, cs) = mock_services_with_schema();
        let service = SchemaService::new(bus, cs);
        let snapshot = service.refresh(mock_connection_id()).await.expect("refresh");

        let table = snapshot
            .objects
            .iter()
            .find_map(|o| match o {
                SchemaObject::Table { name, estimated_rows, .. } if name == "users" => {
                    Some(*estimated_rows)
                }
                _ => None,
            })
            .expect("users table missing");
        assert_eq!(table, Some(42), "estimated_rows must survive the service");

        let view = snapshot
            .objects
            .iter()
            .find_map(|o| match o {
                SchemaObject::View { name, definition, .. } if name == "active_users" => {
                    Some(definition.clone())
                }
                _ => None,
            })
            .expect("active_users view missing");
        assert!(!view.is_empty(), "view definition must survive the service");
    }
```

If the existing mock in that module returns no keywords, fingerprints, `estimated_rows` or view, extend it — the mock is test scaffolding, and these fields are exactly what this task is about. Name the helpers `mock_services_with_schema()` and `mock_connection_id()` if they do not already exist under other names; if they do, use the existing names and say so in your report.

- [ ] **Step 2: Run and watch it fail**

```bash
cargo test -p tempr_services refresh_derives_stable_ids_and_stores_keywords
```

Expected: failure — ids differ between refreshes (they are `SchemaObjectId::new()` today) and `keywords` is empty.

- [ ] **Step 3: Add the storage handle**

```rust
use std::sync::Arc;

use tempr_workspace::Storage;

pub struct SchemaService {
    event_bus: Arc<EventBus>,
    connection_service: Arc<ConnectionService>,
    snapshots: RwLock<HashMap<ConnectionId, Arc<SchemaSnapshot>>>,
    /// Absent when no workspace root is writable: the catalog then lives only
    /// in memory for the session, which is a degradation, never an error.
    storage: Option<Arc<dyn Storage>>,
}

impl SchemaService {
    pub fn new(event_bus: Arc<EventBus>, connection_service: Arc<ConnectionService>) -> Arc<Self> {
        Arc::new(Self {
            event_bus,
            connection_service,
            snapshots: RwLock::new(HashMap::new()),
            storage: None,
        })
    }

    /// Same service, backed by a catalog cache on disk.
    pub fn with_cache(
        event_bus: Arc<EventBus>,
        connection_service: Arc<ConnectionService>,
        storage: Arc<dyn Storage>,
    ) -> Arc<Self> {
        Arc::new(Self {
            event_bus,
            connection_service,
            snapshots: RwLock::new(HashMap::new()),
            storage: Some(storage),
        })
    }

    /// Load this connection's cached snapshot, if any, into memory. Returns
    /// what it loaded so a caller can use the catalog before — or without —
    /// reaching the database.
    pub async fn load_cached(&self, connection_id: ConnectionId) -> Option<Arc<SchemaSnapshot>> {
        let storage = self.storage.as_ref()?;
        let cache = storage.catalog_cache(connection_id);
        match cache.load().await {
            Ok(Some(snapshot)) => {
                let snapshot = Arc::new(snapshot);
                self.snapshots
                    .write()
                    .insert(connection_id, snapshot.clone());
                Some(snapshot)
            }
            Ok(None) => None,
            Err(e) => {
                tracing::warn!(error = %e, "catalog cache load failed; will re-introspect");
                None
            }
        }
    }

    /// Best-effort cache write. Introspection already succeeded, so a failure
    /// here is logged and swallowed — never propagated to the caller.
    async fn save_cached(&self, snapshot: &SchemaSnapshot) {
        let Some(storage) = self.storage.as_ref() else {
            return;
        };
        let cache = storage.catalog_cache(snapshot.connection_id);
        // Skip the write when the content is byte-identical to what is there.
        if let Ok(Some(existing)) = cache.load().await {
            if let (Ok(a), Ok(b)) = (
                tempr_workspace::encode_catalog(&existing),
                tempr_workspace::encode_catalog(snapshot),
            ) {
                if tempr_workspace::content_hash(&a) == tempr_workspace::content_hash(&b) {
                    return;
                }
            }
        }
        if let Err(e) = cache.save(snapshot).await {
            tracing::warn!(error = %e, "catalog cache save failed; catalog stays in memory");
        }
    }
}
```

Add `tempr_workspace.workspace = true` and `tracing.workspace = true` to `crates/tempr_services/Cargo.toml` if they are not already there.

- [ ] **Step 4: Rewrite `refresh`**

Replace the whole method. The two-pass shape stays; what changes is the scope, the id derivation, the pass-through fields, and the three things fetched on one connection:

```rust
    pub async fn refresh(
        &self,
        connection_id: ConnectionId,
    ) -> Result<Arc<SchemaSnapshot>, ServiceError> {
        // One connection, one moment: entries, keywords and fingerprints must
        // describe the same server state or the first diff is already wrong.
        let (entries, keywords, fingerprints) = self
            .connection_service
            .with_metadata_connection_fn(connection_id, |mut conn| async move {
                let entries = conn.snapshot_schema(SchemaScope::SearchPath).await?;
                let keywords = conn.keywords().await?;
                let fingerprints = match conn.schema_fingerprints(SchemaScope::SearchPath).await {
                    Ok(f) => f,
                    // A driver without a sweep simply never refreshes
                    // incrementally; that is not a refresh failure.
                    Err(DriverError::Unsupported(_)) => Vec::new(),
                    Err(e) => return Err(e),
                };
                Ok((entries, keywords, fingerprints))
            })
            .await?;

        let objects = Self::objects_from_entries(connection_id, &entries);
        let fingerprints = fingerprints
            .into_iter()
            .map(|f| SchemaFingerprintRecord {
                kind: domain_kind(f.kind),
                native_id: f.native_id,
                version: f.version,
            })
            .collect();

        let version = self
            .snapshots
            .read()
            .get(&connection_id)
            .map(|s| s.version + 1)
            .unwrap_or(1);

        let snapshot = Arc::new(SchemaSnapshot {
            id: SchemaSnapshotId::new(),
            connection_id,
            version,
            fetched_at: chrono::Utc::now(),
            objects,
            keywords,
            fingerprints,
        });

        self.snapshots
            .write()
            .insert(connection_id, snapshot.clone());
        self.save_cached(&snapshot).await;
        self.event_bus.publish(AppEvent::SchemaRefreshed {
            connection: connection_id,
            snapshot: snapshot.id,
        });
        Ok(snapshot)
    }
```

- [ ] **Step 5: Extract the entry-to-object mapping**

The existing two-pass body becomes a pure function, so Task 6 can reuse it for a partial re-introspection:

```rust
    /// Map driver entries to domain objects, deriving every id from
    /// `(connection, kind, native_id)` so the result is reproducible.
    fn objects_from_entries(
        connection_id: ConnectionId,
        entries: &[SchemaSnapshotEntry],
    ) -> Vec<SchemaObject> {
        // First pass: relations, and a (schema, name) → id map for parents.
        let mut parents: HashMap<(String, String), SchemaObjectId> = HashMap::new();
        let mut objects: Vec<SchemaObject> = Vec::new();

        for entry in entries {
            match entry {
                SchemaSnapshotEntry::Table {
                    native_id,
                    schema,
                    name,
                    estimated_rows,
                } => {
                    let id =
                        SchemaObjectId::derived(connection_id, SchemaObjectKind::Table, *native_id);
                    parents.insert((schema.clone(), name.clone()), id);
                    objects.push(SchemaObject::Table {
                        id,
                        schema: schema.clone(),
                        name: name.clone(),
                        estimated_rows: *estimated_rows,
                    });
                }
                SchemaSnapshotEntry::View {
                    native_id,
                    schema,
                    name,
                    definition,
                } => {
                    let id =
                        SchemaObjectId::derived(connection_id, SchemaObjectKind::View, *native_id);
                    parents.insert((schema.clone(), name.clone()), id);
                    objects.push(SchemaObject::View {
                        id,
                        schema: schema.clone(),
                        name: name.clone(),
                        definition: definition.clone(),
                    });
                }
                _ => {}
            }
        }

        // Second pass: children, resolved against the map above.
        for entry in entries {
            match entry {
                SchemaSnapshotEntry::Column {
                    native_id,
                    parent_schema,
                    parent_table,
                    name,
                    data_type,
                    nullable,
                    ordinal,
                    default,
                } => {
                    let Some(parent_id) = parents.get(&(parent_schema.clone(), parent_table.clone()))
                    else {
                        // A column whose relation is out of scope has no
                        // parent to hang from; dropping it is correct, and
                        // inventing a parent id (as this code used to) is not.
                        tracing::debug!(
                            schema = %parent_schema, table = %parent_table, column = %name,
                            "column skipped: parent relation not in scope"
                        );
                        continue;
                    };
                    objects.push(SchemaObject::Column {
                        id: SchemaObjectId::derived(
                            connection_id,
                            SchemaObjectKind::Column,
                            *native_id,
                        ),
                        parent_id: *parent_id,
                        name: name.clone(),
                        data_type: data_type.clone(),
                        nullable: *nullable,
                        ordinal: *ordinal,
                        default: default.clone(),
                    });
                }
                SchemaSnapshotEntry::Index {
                    native_id,
                    parent_schema,
                    parent_table,
                    name,
                    columns,
                    unique,
                    index_type,
                } => {
                    let Some(parent_table_id) =
                        parents.get(&(parent_schema.clone(), parent_table.clone()))
                    else {
                        continue;
                    };
                    objects.push(SchemaObject::Index {
                        id: SchemaObjectId::derived(
                            connection_id,
                            SchemaObjectKind::Index,
                            *native_id,
                        ),
                        parent_table_id: *parent_table_id,
                        name: name.clone(),
                        columns: columns.clone(),
                        unique: *unique,
                        index_type: *index_type,
                    });
                }
                SchemaSnapshotEntry::Function {
                    native_id,
                    schema,
                    name,
                    parameters,
                    return_type,
                    language,
                } => {
                    objects.push(SchemaObject::Function {
                        id: SchemaObjectId::derived(
                            connection_id,
                            SchemaObjectKind::Function,
                            *native_id,
                        ),
                        schema: schema.clone(),
                        name: name.clone(),
                        parameters: parameters.clone(),
                        return_type: return_type.clone(),
                        language: language.clone(),
                    });
                }
                _ => {}
            }
        }
        objects
    }
```

Note `index_type: *index_type` will not compile if the field is a `String` — use `index_type.clone()`. Check the domain type and match it.

- [ ] **Step 6: Add the kind mapping helper**

```rust
/// The sweep cannot tell a table from a view — both are `pg_class` rows —
/// while the domain can. Mapping `Relation` to `Table` is safe because a
/// fingerprint key is only ever compared against another fingerprint key.
fn domain_kind(kind: ObjectKind) -> SchemaObjectKind {
    match kind {
        ObjectKind::Relation => SchemaObjectKind::Table,
        ObjectKind::Column => SchemaObjectKind::Column,
        ObjectKind::Index => SchemaObjectKind::Index,
        ObjectKind::Function => SchemaObjectKind::Function,
    }
}
```

- [ ] **Step 7: Wire the binary**

In `crates/tempr/src/main.rs`, where the services are built, construct storage rooted at the workspace path D24 already resolves (`workspace_manifest_path()`'s parent) and use it:

```rust
    let storage: Arc<dyn tempr_workspace::Storage> = Arc::new(
        tempr_workspace::FileSystemStorage::new(
            workspace_manifest_path()
                .parent()
                .map(std::path::Path::to_path_buf)
                .unwrap_or_else(|| std::path::PathBuf::from(".")),
        ),
    );
    let schema = SchemaService::with_cache(bus.clone(), connection.clone(), storage);
```

Keep everything else in `build_services` as it is.

- [ ] **Step 8: Run the tests**

```bash
cargo test --workspace
```

Expected: PASS, including the two new tests.

- [ ] **Step 9: Commit**

```bash
git add crates/tempr_services/ crates/tempr/src/main.rs
git commit -m "feat(services): derived identity, keywords and catalog cache writes on refresh"
```

---

## Task 6: Incremental refresh

**Files:**
- Modify: `crates/tempr_services/src/schema.rs`
- Test: same file

**Interfaces:**
- Consumes: Task 5's `objects_from_entries`, `domain_kind`, `save_cached`
- Produces:
  ```rust
  pub struct SchemaDelta { pub added: Vec<(SchemaObjectKind, u64)>,
                           pub changed: Vec<(SchemaObjectKind, u64)>,
                           pub dropped: Vec<(SchemaObjectKind, u64)> }
  impl SchemaService {
      pub fn diff_fingerprints(cached: &[SchemaFingerprintRecord], swept: &[SchemaFingerprint]) -> SchemaDelta;
      pub async fn refresh_incremental(&self, connection_id: ConnectionId) -> Result<Arc<SchemaSnapshot>, ServiceError>;
  }
  ```

- [ ] **Step 1: Write the failing tests**

```rust
    #[test]
    fn diff_classifies_changed_new_and_dropped() {
        let cached = vec![
            SchemaFingerprintRecord { kind: SchemaObjectKind::Table, native_id: 1, version: 10 },
            SchemaFingerprintRecord { kind: SchemaObjectKind::Table, native_id: 2, version: 20 },
            SchemaFingerprintRecord { kind: SchemaObjectKind::Column, native_id: 100, version: 30 },
        ];
        let swept = vec![
            SchemaFingerprint { native_id: 1, kind: ObjectKind::Relation, version: 10 }, // unchanged
            SchemaFingerprint { native_id: 2, kind: ObjectKind::Relation, version: 21 }, // changed
            SchemaFingerprint { native_id: 3, kind: ObjectKind::Relation, version: 40 }, // new
            // column 100 absent → dropped
        ];

        let delta = SchemaService::diff_fingerprints(&cached, &swept);
        assert_eq!(delta.changed, vec![(SchemaObjectKind::Table, 2)]);
        assert_eq!(delta.added, vec![(SchemaObjectKind::Table, 3)]);
        assert_eq!(delta.dropped, vec![(SchemaObjectKind::Column, 100)]);
        assert_eq!(delta.touched(), 3);
        assert!(!delta.is_empty());
    }

    #[test]
    fn an_identical_sweep_is_an_empty_delta() {
        let cached = vec![SchemaFingerprintRecord {
            kind: SchemaObjectKind::Table,
            native_id: 1,
            version: 10,
        }];
        let swept = vec![SchemaFingerprint {
            native_id: 1,
            kind: ObjectKind::Relation,
            version: 10,
        }];
        let delta = SchemaService::diff_fingerprints(&cached, &swept);
        assert!(delta.is_empty());
        assert_eq!(delta.touched(), 0);
    }

    #[test]
    fn a_column_id_never_matches_a_relation_id_with_the_same_number() {
        // The pair (kind, native_id) is the key precisely because these two
        // numbers can coincide on a database whose OID counter has wrapped.
        let cached = vec![SchemaFingerprintRecord {
            kind: SchemaObjectKind::Column,
            native_id: 16384,
            version: 1,
        }];
        let swept = vec![SchemaFingerprint {
            native_id: 16384,
            kind: ObjectKind::Relation,
            version: 1,
        }];
        let delta = SchemaService::diff_fingerprints(&cached, &swept);
        assert_eq!(delta.added, vec![(SchemaObjectKind::Table, 16384)]);
        assert_eq!(delta.dropped, vec![(SchemaObjectKind::Column, 16384)]);
    }

    #[tokio::test]
    async fn incremental_without_a_cached_snapshot_falls_back_to_full() {
        let (bus, cs) = mock_services_with_schema();
        let service = SchemaService::new(bus, cs);
        let snapshot = service
            .refresh_incremental(mock_connection_id())
            .await
            .expect("incremental with no cache");
        assert_eq!(snapshot.version, 1, "a full refresh produced version 1");
        assert!(!snapshot.objects.is_empty());
    }
```

- [ ] **Step 2: Run and watch them fail**

```bash
cargo test -p tempr_services diff_classifies
```

Expected: compile error — `diff_fingerprints` does not exist.

- [ ] **Step 3: Implement the delta type and the diff**

```rust
/// What a fingerprint sweep says changed since the cached snapshot.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct SchemaDelta {
    pub added: Vec<(SchemaObjectKind, u64)>,
    pub changed: Vec<(SchemaObjectKind, u64)>,
    pub dropped: Vec<(SchemaObjectKind, u64)>,
}

impl SchemaDelta {
    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.changed.is_empty() && self.dropped.is_empty()
    }

    /// Objects touched, weighed against the full-refresh threshold.
    pub fn touched(&self) -> usize {
        self.added.len() + self.changed.len() + self.dropped.len()
    }
}

impl SchemaService {
    /// Pure diff of a cached fingerprint list against a fresh sweep.
    pub fn diff_fingerprints(
        cached: &[SchemaFingerprintRecord],
        swept: &[SchemaFingerprint],
    ) -> SchemaDelta {
        let mut before: HashMap<(SchemaObjectKind, u64), u64> = cached
            .iter()
            .map(|f| ((f.kind, f.native_id), f.version))
            .collect();

        let mut delta = SchemaDelta::default();
        for f in swept {
            let key = (domain_kind(f.kind), f.native_id);
            match before.remove(&key) {
                Some(version) if version == f.version => {}
                Some(_) => delta.changed.push(key),
                None => delta.added.push(key),
            }
        }
        delta.dropped = before.into_keys().collect();
        delta.added.sort_unstable();
        delta.changed.sort_unstable();
        delta.dropped.sort_unstable();
        delta
    }
}
```

- [ ] **Step 4: Implement `refresh_incremental`**

```rust
    /// Sweep fingerprints and re-introspect only the schemas that moved.
    ///
    /// Falls back to a full refresh when there is nothing to diff against
    /// (no cached snapshot, or a cache with no fingerprints), when the driver
    /// has no sweep, when an object appeared that the cache has never seen
    /// (its schema and name are unknown, so nothing can be scoped to it), or
    /// when more than 40% of known objects moved — past that a single full
    /// introspection is the cheaper query.
    pub async fn refresh_incremental(
        &self,
        connection_id: ConnectionId,
    ) -> Result<Arc<SchemaSnapshot>, ServiceError> {
        let cached = match self.snapshot(connection_id) {
            Some(snapshot) => Some(snapshot),
            None => self.load_cached(connection_id).await,
        };
        let Some(cached) = cached.filter(|s| !s.fingerprints.is_empty()) else {
            return self.refresh(connection_id).await;
        };

        let swept = match self
            .connection_service
            .with_metadata_connection_fn(connection_id, |mut conn| async move {
                conn.schema_fingerprints(SchemaScope::SearchPath).await
            })
            .await
        {
            Ok(swept) => swept,
            Err(ServiceError::QueryFailed(_)) | Err(ServiceError::NotConnected) => {
                return self.refresh(connection_id).await;
            }
            Err(e) => return Err(e),
        };

        let delta = Self::diff_fingerprints(&cached.fingerprints, &swept);
        if delta.is_empty() {
            // Nothing moved: no new snapshot, no event, no cache rewrite.
            return Ok(cached);
        }
        if !delta.added.is_empty() || delta.touched() * 5 > cached.fingerprints.len() * 2 {
            return self.refresh(connection_id).await;
        }

        // Every changed object is known, so its schema is known.
        let mut schemas: Vec<String> = delta
            .changed
            .iter()
            .filter_map(|(kind, native_id)| {
                let id = SchemaObjectId::derived(connection_id, *kind, *native_id);
                cached.objects.iter().find_map(|o| match o {
                    SchemaObject::Table { id: oid, schema, .. }
                    | SchemaObject::View { id: oid, schema, .. }
                    | SchemaObject::Function { id: oid, schema, .. }
                        if *oid == id =>
                    {
                        Some(schema.clone())
                    }
                    _ => None,
                })
            })
            .collect();
        schemas.sort();
        schemas.dedup();
        if schemas.is_empty() {
            // Changed columns or indexes whose parent we could not resolve.
            return self.refresh(connection_id).await;
        }

        let mut entries = Vec::new();
        for schema in &schemas {
            let scope = SchemaScope::Schema(schema.clone());
            let mut part = self
                .connection_service
                .with_metadata_connection_fn(connection_id, |mut conn| async move {
                    conn.snapshot_schema(scope).await
                })
                .await?;
            entries.append(&mut part);
        }
        let refreshed = Self::objects_from_entries(connection_id, &entries);

        // Keep every object outside the touched schemas, minus the dropped.
        let dropped: std::collections::HashSet<SchemaObjectId> = delta
            .dropped
            .iter()
            .map(|(kind, native_id)| {
                SchemaObjectId::derived(connection_id, *kind, *native_id)
            })
            .collect();
        let refreshed_ids: std::collections::HashSet<SchemaObjectId> =
            refreshed.iter().map(|o| o.id()).collect();
        let mut objects: Vec<SchemaObject> = cached
            .objects
            .iter()
            .filter(|o| !dropped.contains(&o.id()) && !refreshed_ids.contains(&o.id()))
            .cloned()
            .collect();
        objects.extend(refreshed);

        let snapshot = Arc::new(SchemaSnapshot {
            id: SchemaSnapshotId::new(),
            connection_id,
            version: cached.version + 1,
            fetched_at: chrono::Utc::now(),
            objects,
            keywords: cached.keywords.clone(),
            fingerprints: swept
                .into_iter()
                .map(|f| SchemaFingerprintRecord {
                    kind: domain_kind(f.kind),
                    native_id: f.native_id,
                    version: f.version,
                })
                .collect(),
        });

        self.snapshots
            .write()
            .insert(connection_id, snapshot.clone());
        self.save_cached(&snapshot).await;
        self.event_bus.publish(AppEvent::SchemaRefreshed {
            connection: connection_id,
            snapshot: snapshot.id,
        });
        Ok(snapshot)
    }
```

Two things to check while implementing, and to report on:
- The `ServiceError` variants matched in the sweep fallback must be the ones `with_metadata_connection_fn` actually returns for an unsupported driver call. Read `ServiceError` and the connection service before assuming; if `DriverError::Unsupported` surfaces as a different variant, match that one.
- Objects living in a touched schema but no longer returned by the re-introspection must not survive. The `refreshed_ids` filter above only removes objects that came back; add a schema-based filter if the domain objects carry enough information to identify their schema, and say in your report which you did.

- [ ] **Step 5: Run the tests**

```bash
cargo test -p tempr_services
```

Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/tempr_services/
git commit -m "feat(services): incremental schema refresh from a fingerprint diff"
```

---

## Task 7: Live-server tests, docs and hand-back

**Files:**
- Modify: `crates/tempr/tests/integration.rs`
- Modify: `docs/DECISIONS.md`, `docs/07-storage.md`, `docs/05-services.md`, `docs/PROGRESS.md`, `docs/TODO.md`

- [ ] **Step 1: Write the live-server tests**

```rust
#[tokio::test]
#[ignore = "requires DATABASE_URL env var pointing to a live PostgreSQL instance"]
async fn pg_object_ids_survive_a_second_refresh() {
    let (bus, cs) = setup_pg_cs();
    let id = connect_test_pg(&cs).await;
    cs.with_metadata_connection_fn(id, |mut conn| async move {
        conn.execute("DROP TABLE IF EXISTS id_stability", &[]).await?;
        conn.execute("CREATE TABLE id_stability (id int, label text)", &[]).await
    })
    .await
    .expect("setup");

    let service = SchemaService::new(bus, cs.clone());
    let first = service.refresh(id).await.expect("first refresh");
    let second = service.refresh(id).await.expect("second refresh");

    let ids = |s: &SchemaSnapshot| -> Vec<SchemaObjectId> {
        let mut v: Vec<_> = s.objects.iter().map(|o| o.id()).collect();
        v.sort_by_key(|i| i.0);
        v
    };
    assert_eq!(ids(&first), ids(&second), "ids must be reproducible");
    assert!(!first.keywords.is_empty(), "keywords came from the server");
    assert!(!first.fingerprints.is_empty(), "fingerprints were stored");

    cs.with_metadata_connection_fn(id, |mut conn| async move {
        conn.execute("DROP TABLE id_stability", &[]).await
    })
    .await
    .expect("cleanup");
}

#[tokio::test]
#[ignore = "requires DATABASE_URL env var pointing to a live PostgreSQL instance"]
async fn pg_incremental_refresh_keeps_untouched_ids_and_sees_the_new_column() {
    let (bus, cs) = setup_pg_cs();
    let id = connect_test_pg(&cs).await;
    cs.with_metadata_connection_fn(id, |mut conn| async move {
        conn.execute("DROP TABLE IF EXISTS inc_touched", &[]).await?;
        conn.execute("DROP TABLE IF EXISTS inc_untouched", &[]).await?;
        conn.execute("CREATE TABLE inc_touched (id int)", &[]).await?;
        conn.execute("CREATE TABLE inc_untouched (id int)", &[]).await
    })
    .await
    .expect("setup");

    let service = SchemaService::new(bus, cs.clone());
    let before = service.refresh(id).await.expect("full refresh");

    cs.with_metadata_connection_fn(id, |mut conn| async move {
        conn.execute("ALTER TABLE inc_touched ADD COLUMN label text", &[]).await
    })
    .await
    .expect("alter");

    let after = service.refresh_incremental(id).await.expect("incremental");
    assert_eq!(after.version, before.version + 1);

    let column_named = |s: &SchemaSnapshot, want: &str| {
        s.objects.iter().any(|o| matches!(o, SchemaObject::Column { name, .. } if name == want))
    };
    assert!(!column_named(&before, "label"), "column did not exist yet");
    assert!(column_named(&after, "label"), "incremental refresh missed the new column");

    let untouched_id = |s: &SchemaSnapshot| {
        s.objects.iter().find_map(|o| match o {
            SchemaObject::Table { id, name, .. } if name == "inc_untouched" => Some(*id),
            _ => None,
        })
    };
    assert_eq!(
        untouched_id(&before),
        untouched_id(&after),
        "an untouched table must keep its identity"
    );

    cs.with_metadata_connection_fn(id, |mut conn| async move {
        conn.execute("DROP TABLE inc_touched", &[]).await?;
        conn.execute("DROP TABLE inc_untouched", &[]).await
    })
    .await
    .expect("cleanup");
}

#[tokio::test]
#[ignore = "requires DATABASE_URL env var pointing to a live PostgreSQL instance"]
async fn pg_catalog_is_readable_without_the_server() {
    // The phase's offline criterion: introspect once, then serve the catalog
    // from disk with a service that never talks to the database.
    let dir = tempfile::tempdir().expect("tempdir");
    let storage: std::sync::Arc<dyn tempr_workspace::Storage> =
        std::sync::Arc::new(tempr_workspace::FileSystemStorage::new(dir.path()));
    storage.init_workspace_dir().await.expect("init workspace dir");

    let (bus, cs) = setup_pg_cs();
    let id = connect_test_pg(&cs).await;
    let service = SchemaService::with_cache(bus.clone(), cs.clone(), storage.clone());
    let written = service.refresh(id).await.expect("refresh");
    assert!(!written.objects.is_empty());

    // A brand-new service over the same storage, and a connection service
    // pointing nowhere: nothing here can reach PostgreSQL.
    let offline_cs = ConnectionService::new(bus.clone());
    let offline = SchemaService::with_cache(bus, std::sync::Arc::new(offline_cs), storage);
    let loaded = offline.load_cached(id).await.expect("cache hit");

    assert_eq!(loaded.id, written.id);
    assert_eq!(loaded.objects.len(), written.objects.len());
    assert_eq!(loaded.keywords, written.keywords);
    assert!(offline.snapshot(id).is_some(), "loading populates the in-memory map");
}
```

`ConnectionService::new` may take different arguments — read it and adapt; the point of the third test is only that the offline service has no route to the database.

- [ ] **Step 2: Record D25 and D26**

**D25 — Schema object identity is derived, not random (2026-09-08).** UUIDv5 over the connection id as namespace and `(kind discriminant, native_id)` as name. Why: a cache that cannot be diffed is a cache that must be thrown away on every refresh; kind is in the key because a packed column id can numerically equal a relation OID after OID wraparound. Consequences: renumbering `SchemaObjectKind::discriminant` orphans every cache file in the wild; drivers with no stable native id hash their qualified name into the same field and get rename-as-delete-plus-insert.

**D26 — Incremental refresh diffs an `(oid, xmin)` sweep (2026-09-08).** The driver's sweep is one query; the service diffs it against the fingerprints stored with the cached snapshot and re-introspects only the affected schemas. Full refresh is the fallback in four cases: no cached fingerprints, no driver support, an object the cache has never seen, and more than 40% of objects touched. Why: PostgreSQL has no change feed, event triggers would write into the user's database, and a frozen `xmin` produces a false positive (a harmless re-introspect) rather than a missed change. Consequences: re-introspection is per schema, not per object, because catalog queries are shaped by schema and name while the sweep returns only OIDs.

- [ ] **Step 3: Update the reference docs**

`docs/07-storage.md` — mark OD#1 resolved, pointing at D27, and replace the sketched `CatalogCache` trait with the real `CatalogCacheFile` / `Storage::catalog_cache` signatures, the `.tcat` header layout, and the discard-on-mismatch rule.

`docs/05-services.md` — `SchemaService`'s real surface (`new`, `with_cache`, `refresh`, `refresh_incremental`, `load_cached`, `snapshot`, `version`), and the claim about persisting to the catalog cache, now true.

- [ ] **Step 4: Update the living docs**

`docs/PROGRESS.md` — status block, one session-log row, decisions-log rows linking D25-D27. Check exactly the two Phase 3 boxes this stage completes: "Catalog cache loads full schema metadata from PostgreSQL and caches it locally" and "Cache refreshes incrementally; full refresh available on demand". Leave every other box unchecked. Put the numbers you actually observed in the Verified line.

`docs/TODO.md` — close the `estimated_rows`/`definition` row and the `SearchPath`-not-used row; add cache eviction (`max_cache_size`, LRU by last access — 07-storage's own follow-up) and a per-connection catalog scope setting.

- [ ] **Step 5: Full verification**

```bash
export DATABASE_URL='postgres://tempr:tempr@localhost:55432/tempr?sslmode=disable'
export DATABASE_URL_TLS='postgres://tempr:tempr@localhost:55433/tempr'
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo test --workspace -- --ignored
cargo deny check
```

If the TLS container will not start, recreate it — its certs live in a session scratchpad and do not survive (the recipe is in the PROGRESS decisions log).

- [ ] **Step 6: Commit and hand back**

```bash
git add docs/ crates/tempr/tests/integration.rs
git commit -m "docs: catalog persistence decisions and Phase 3 stage 2 status"
```

Do not open the PR — report the commit range and the verification output instead.


---

## Notes for the implementer

**Why identity is a hash and not the OID itself.** `SchemaObjectId` is a `Uuid` across the whole domain, and widening it to carry an engine-specific integer would touch every consumer. UUIDv5 keeps one id type, makes the derivation reproducible in any process, and folds the connection in so two databases with identical OIDs never collide.

**Why the diff maps `Relation` to `Table`.** The sweep cannot distinguish a table from a view — both are `pg_class` rows — while the domain can. Keying both as `Table` is safe because the pair `(kind, native_id)` is only ever compared against itself; a view's OID never appears as a table's OID. If a later stage needs the distinction, the sweep must report it, not the diff infer it.

**What "incremental" honestly means here.** The sweep is one query; the re-introspection is scoped per schema, not per object, because PostgreSQL's catalog queries are shaped by schema and name and the sweep returns only OIDs. A single-column change therefore re-reads its whole schema. That is still far less than a full refresh on a multi-schema database, and the fallbacks keep the pathological cases honest rather than slow.

**The cache never blocks a refresh.** Every cache operation is best-effort: a missing directory, a read-only filesystem, or a corrupt file all end in the same place — introspect from the server and carry on. If you find yourself propagating a cache error to the caller, re-read this paragraph.
