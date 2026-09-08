//! The on-disk catalog cache (`.tcat`): a versioned header plus a bincode
//! body (docs/07-storage.md → catalog cache, D27). The cache is derived data,
//! so every decode failure is a discard, never an error the user sees.
//!
//! `SchemaObject` is `#[serde(tag = "kind", ...)]` (internally tagged) for
//! its JSON representation elsewhere in the tree. Internally-tagged enums
//! need the deserializer to buffer arbitrary content to find the tag before
//! it knows which variant to build — that requires `deserialize_any`, which
//! bincode's `Deserializer` does not implement (non-self-describing formats
//! can't support it; verified against bincode 2.0.1, which fails with
//! `Serde(AnyNotSupported)`). `CatalogObject` below is a private mirror of
//! `SchemaObject` with the same variants and fields in bincode's default
//! *externally tagged* representation, which decodes directly with no
//! buffering. It exists only as this module's wire shape — `SchemaObject`
//! itself, and its JSON format, are untouched.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tempr_domain::{
    ConnectionId, SchemaFingerprintRecord, SchemaObject, SchemaObjectId, SchemaSnapshot,
    SchemaSnapshotId,
};

use crate::error::WorkspaceError;

pub const CATALOG_MAGIC: [u8; 4] = *b"TCAT";
/// Bump on ANY layout change: older readers discard what they cannot parse.
pub const CATALOG_FORMAT_VERSION: u16 = 1;
const HEADER_LEN: usize = 4 + 2 + 2 + 8;

/// Bincode-shaped mirror of `SchemaSnapshot` — see the module docs. Every
/// field but `objects` is a domain type used as-is: `Uuid`-backed ids,
/// `DateTime<Utc>` and `SchemaFingerprintRecord` (whose `SchemaObjectKind` is
/// a plain, un-tagged fieldless enum) all round-trip through bincode's serde
/// bridge without issue.
#[derive(Debug, Serialize, Deserialize)]
struct CatalogSnapshot {
    id: SchemaSnapshotId,
    connection_id: ConnectionId,
    version: u64,
    fetched_at: DateTime<Utc>,
    objects: Vec<CatalogObject>,
    keywords: Vec<String>,
    fingerprints: Vec<SchemaFingerprintRecord>,
}

/// Bincode-shaped mirror of `SchemaObject`, field-for-field identical, minus
/// the internal tag. See the module docs.
#[derive(Debug, Serialize, Deserialize)]
enum CatalogObject {
    Table {
        id: SchemaObjectId,
        schema: String,
        name: String,
        estimated_rows: Option<u64>,
    },
    View {
        id: SchemaObjectId,
        schema: String,
        name: String,
        definition: String,
    },
    Column {
        id: SchemaObjectId,
        parent_id: SchemaObjectId,
        name: String,
        data_type: String,
        nullable: bool,
        ordinal: usize,
        default: Option<String>,
    },
    Index {
        id: SchemaObjectId,
        parent_table_id: SchemaObjectId,
        name: String,
        columns: Vec<String>,
        unique: bool,
        index_type: String,
    },
    Function {
        id: SchemaObjectId,
        schema: String,
        name: String,
        parameters: Vec<(String, String)>,
        return_type: String,
        language: String,
    },
}

impl From<&SchemaObject> for CatalogObject {
    fn from(obj: &SchemaObject) -> Self {
        match obj {
            SchemaObject::Table {
                id,
                schema,
                name,
                estimated_rows,
            } => CatalogObject::Table {
                id: *id,
                schema: schema.clone(),
                name: name.clone(),
                estimated_rows: *estimated_rows,
            },
            SchemaObject::View {
                id,
                schema,
                name,
                definition,
            } => CatalogObject::View {
                id: *id,
                schema: schema.clone(),
                name: name.clone(),
                definition: definition.clone(),
            },
            SchemaObject::Column {
                id,
                parent_id,
                name,
                data_type,
                nullable,
                ordinal,
                default,
            } => CatalogObject::Column {
                id: *id,
                parent_id: *parent_id,
                name: name.clone(),
                data_type: data_type.clone(),
                nullable: *nullable,
                ordinal: *ordinal,
                default: default.clone(),
            },
            SchemaObject::Index {
                id,
                parent_table_id,
                name,
                columns,
                unique,
                index_type,
            } => CatalogObject::Index {
                id: *id,
                parent_table_id: *parent_table_id,
                name: name.clone(),
                columns: columns.clone(),
                unique: *unique,
                index_type: index_type.clone(),
            },
            SchemaObject::Function {
                id,
                schema,
                name,
                parameters,
                return_type,
                language,
            } => CatalogObject::Function {
                id: *id,
                schema: schema.clone(),
                name: name.clone(),
                parameters: parameters.clone(),
                return_type: return_type.clone(),
                language: language.clone(),
            },
        }
    }
}

impl From<CatalogObject> for SchemaObject {
    fn from(obj: CatalogObject) -> Self {
        match obj {
            CatalogObject::Table {
                id,
                schema,
                name,
                estimated_rows,
            } => SchemaObject::Table {
                id,
                schema,
                name,
                estimated_rows,
            },
            CatalogObject::View {
                id,
                schema,
                name,
                definition,
            } => SchemaObject::View {
                id,
                schema,
                name,
                definition,
            },
            CatalogObject::Column {
                id,
                parent_id,
                name,
                data_type,
                nullable,
                ordinal,
                default,
            } => SchemaObject::Column {
                id,
                parent_id,
                name,
                data_type,
                nullable,
                ordinal,
                default,
            },
            CatalogObject::Index {
                id,
                parent_table_id,
                name,
                columns,
                unique,
                index_type,
            } => SchemaObject::Index {
                id,
                parent_table_id,
                name,
                columns,
                unique,
                index_type,
            },
            CatalogObject::Function {
                id,
                schema,
                name,
                parameters,
                return_type,
                language,
            } => SchemaObject::Function {
                id,
                schema,
                name,
                parameters,
                return_type,
                language,
            },
        }
    }
}

impl From<&SchemaSnapshot> for CatalogSnapshot {
    fn from(snapshot: &SchemaSnapshot) -> Self {
        CatalogSnapshot {
            id: snapshot.id,
            connection_id: snapshot.connection_id,
            version: snapshot.version,
            fetched_at: snapshot.fetched_at,
            objects: snapshot.objects.iter().map(CatalogObject::from).collect(),
            keywords: snapshot.keywords.clone(),
            fingerprints: snapshot.fingerprints.clone(),
        }
    }
}

impl From<CatalogSnapshot> for SchemaSnapshot {
    fn from(snapshot: CatalogSnapshot) -> Self {
        SchemaSnapshot {
            id: snapshot.id,
            connection_id: snapshot.connection_id,
            version: snapshot.version,
            fetched_at: snapshot.fetched_at,
            objects: snapshot
                .objects
                .into_iter()
                .map(SchemaObject::from)
                .collect(),
            keywords: snapshot.keywords,
            fingerprints: snapshot.fingerprints,
        }
    }
}

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

/// Hash of everything in a snapshot that describes the database, ignoring the
/// per-refresh bookkeeping (`id`, `version`, `fetched_at`). Two snapshots of an
/// unchanged schema hash the same, which is what lets a writer skip a
/// pointless rewrite.
pub fn snapshot_content_hash(snapshot: &SchemaSnapshot) -> Result<u64, WorkspaceError> {
    let objects: Vec<CatalogObject> = snapshot.objects.iter().map(CatalogObject::from).collect();
    let body = bincode::serde::encode_to_vec(
        (&objects, &snapshot.keywords, &snapshot.fingerprints),
        bincode::config::standard(),
    )
    .map_err(|e| WorkspaceError::Corrupted {
        reason: format!("catalog content hash encode failed: {e}"),
    })?;
    Ok(content_hash(&body))
}

/// Encode a snapshot into header + body.
pub fn encode_catalog(snapshot: &SchemaSnapshot) -> Result<Vec<u8>, WorkspaceError> {
    let wire = CatalogSnapshot::from(snapshot);
    let body = bincode::serde::encode_to_vec(&wire, bincode::config::standard()).map_err(|e| {
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
    match bincode::serde::decode_from_slice::<CatalogSnapshot, _>(body, bincode::config::standard())
    {
        Ok((wire, _)) => Ok(Some(SchemaSnapshot::from(wire))),
        Err(_) => Ok(None),
    }
}

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
        let back = decode_catalog(&bytes)
            .expect("decode")
            .expect("not discarded");
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
        assert!(
            decode_catalog(&bytes[..bytes.len() / 2])
                .expect("no error")
                .is_none()
        );
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

    #[test]
    fn content_hash_ignores_bookkeeping_but_not_content() {
        let a = sample();
        let mut b = a.clone();
        b.id = SchemaSnapshotId::new();
        b.version = a.version + 5;
        b.fetched_at = a.fetched_at + chrono::Duration::hours(3);
        assert_eq!(
            snapshot_content_hash(&a).expect("hash a"),
            snapshot_content_hash(&b).expect("hash b"),
            "id, version and fetched_at must not affect the content hash"
        );

        let mut c = a.clone();
        c.keywords.push("merge".to_string());
        assert_ne!(
            snapshot_content_hash(&a).expect("hash a"),
            snapshot_content_hash(&c).expect("hash c"),
            "a content change must change the hash"
        );
    }
}
