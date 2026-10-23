//! The in-memory catalog (spec §4.1): a resolution-shaped, immutable arena
//! built from a `SchemaSnapshot`. The snapshot stays the wire and disk shape;
//! this structure is derived data, rebuilt wholesale on every refresh and
//! swapped behind an `Arc` once the semantic engine exists.
//!
//! Layout: objects live in one arena `Vec<CatalogEntry>` — relations first,
//! then functions, then columns grouped contiguously per relation in
//! `ordinal` order, then indexes — and every map holds `u32` indices into it.
//! Columns per relation are therefore a slice, not a scan.
//!
//! Identifiers follow PostgreSQL: an *unquoted* identifier folds to lower
//! case (matched through the folded maps), a *quoted* one is exact
//! (matched through the case-sensitive map). Both point into the same arena.
//!
//! Internal-layout note (spec §4.1 refinement): `columns_of` is a
//! `Vec<Range<u32>>` indexed by relation arena index rather than a
//! `HashMap` — relation indices are dense `0..n_relations`, so the vector
//! needs no hashing on the hot path. No public shape changes.
//!
//! `SchemaObject::Database` is recorded in the snapshot but deliberately has
//! no arena slot: it names the catalog, and SQL text cannot resolve a
//! database, so it must never become a candidate or a hover target.

use std::ops::Range;
use std::time::Instant;

use tempr_domain::ids::{ConnectionId, SchemaObjectId, SchemaSnapshotId};
use tempr_domain::schema::{SchemaObject, SchemaObjectKind, SchemaSnapshot};

/// One object of the cached catalog, keyed by arena index. Field shapes
/// mirror `SchemaObject`; parent references (columns, indexes) resolve to
/// the parent's arena index so no id lookup is needed on the hot path.
#[derive(Debug, Clone, PartialEq)]
pub enum CatalogEntry {
    Table {
        id: SchemaObjectId,
        schema: Box<str>,
        name: Box<str>,
        estimated_rows: Option<u64>,
    },
    View {
        id: SchemaObjectId,
        schema: Box<str>,
        name: Box<str>,
        definition: Box<str>,
    },
    Column {
        id: SchemaObjectId,
        parent: u32,
        name: Box<str>,
        data_type: Box<str>,
        nullable: bool,
        ordinal: usize,
        default: Option<Box<str>>,
    },
    Index {
        id: SchemaObjectId,
        parent: u32,
        name: Box<str>,
        columns: Vec<Box<str>>,
        unique: bool,
        index_type: Box<str>,
    },
    Function {
        id: SchemaObjectId,
        schema: Box<str>,
        name: Box<str>,
        parameters: Vec<(Box<str>, Box<str>)>,
        return_type: Box<str>,
        language: Box<str>,
    },
}

impl CatalogEntry {
    pub fn id(&self) -> SchemaObjectId {
        match self {
            CatalogEntry::Table { id, .. }
            | CatalogEntry::View { id, .. }
            | CatalogEntry::Column { id, .. }
            | CatalogEntry::Index { id, .. }
            | CatalogEntry::Function { id, .. } => *id,
        }
    }

    pub fn kind(&self) -> SchemaObjectKind {
        match self {
            CatalogEntry::Table { .. } => SchemaObjectKind::Table,
            CatalogEntry::View { .. } => SchemaObjectKind::View,
            CatalogEntry::Column { .. } => SchemaObjectKind::Column,
            CatalogEntry::Index { .. } => SchemaObjectKind::Index,
            CatalogEntry::Function { .. } => SchemaObjectKind::Function,
        }
    }

    /// The object's name: relation/column/index/function name, as reported by
    /// the server's catalog (quoted-created mixed-case names stay mixed here;
    /// folding happens only during *lookup*).
    pub fn name(&self) -> &str {
        match self {
            CatalogEntry::Table { name, .. }
            | CatalogEntry::View { name, .. }
            | CatalogEntry::Column { name, .. }
            | CatalogEntry::Index { name, .. }
            | CatalogEntry::Function { name, .. } => name,
        }
    }

    /// The schema the object lives in. `None` for columns and indexes, which
    /// hang off their parent relation instead.
    pub fn schema(&self) -> Option<&str> {
        match self {
            CatalogEntry::Table { schema, .. }
            | CatalogEntry::View { schema, .. }
            | CatalogEntry::Function { schema, .. } => Some(schema),
            CatalogEntry::Column { .. } | CatalogEntry::Index { .. } => None,
        }
    }

    /// The parent relation's arena index, for columns and indexes.
    pub fn parent(&self) -> Option<u32> {
        match self {
            CatalogEntry::Column { parent, .. } | CatalogEntry::Index { parent, .. } => {
                Some(*parent)
            }
            _ => None,
        }
    }
}

/// Cheap per-cache summary for status lines and probes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CatalogStats {
    pub relations: usize,
    pub columns: usize,
    pub indexes: usize,
    pub functions: usize,
    pub schemas: usize,
    pub keywords: usize,
    pub snapshot_id: SchemaSnapshotId,
    /// When the cache was built from its snapshot (process-local, for probes).
    pub built_at: Instant,
}

/// An immutable, resolution-shaped view of one connection's schema. Build a
/// fresh one per snapshot; never mutate. Readers copy the `Arc` and work
/// lock-free against this structure (the swap lives with the engine, stage 4).
///
/// Also cheap to build: from_snapshot on a 10 000-object snapshot.
#[derive(Debug, Clone)]
pub struct CatalogCache {
    snapshot_id: SchemaSnapshotId,
    connection_id: ConnectionId,
    snapshot_version: u64,

    /// Arena; indices into here are `u32`.
    objects: Vec<CatalogEntry>,
    /// `SchemaObjectId` → arena index. A count repeated as many times as
    /// needed for the column-covered object.
    by_id: std::collections::HashMap<SchemaObjectId, u32>,
    /// Number of relations (tables + views) at the arena head; columns and
    /// indexes come after functions and refer into this range.
    n_relations: usize,
    /// Exact `(schema, name)` → arena index, case-sensitive (quoted path).
    relations_by_name: std::collections::HashMap<(Box<str>, Box<str>), u32>,
    /// Folded `(schema, name)` → arena indices; more than one means the
    /// folded name is ambiguous (e.g. `users` and quoted `"Users"` coexist),
    /// which the analyzer reports — the catalog does not guess.
    relations_by_folded_name: std::collections::HashMap<(Box<str>, Box<str>), Vec<u32>>,
    /// Relation arena index → range of contiguous column indices, ordinal
    /// order. Empty range = no columns introspected.
    columns_of: Vec<Range<u32>>,
    /// Folded name → arena index for prefix search, sorted, relations and
    /// functions only (completion's table/function candidates; column
    /// candidates come from scope bindings and `columns_of`).
    name_order: Vec<(Box<str>, u32)>,
    /// Distinct schemas, sorted.
    schemas: Vec<Box<str>>,
    keywords: Vec<String>,
    stats: CatalogStats,
}

// public API + tests arrive with tasks 1-3

fn fold(s: &str) -> Box<str> {
    // PostgreSQL folds *unquoted* identifiers per its own rules; full Unicode
    // folding here matches the ASCII case exactly and degrades reasonably for
    // non-ASCII names. The exact maps still win for quoted identifiers.
    s.to_lowercase().into_boxed_str()
}

type RelKey = (Box<str>, Box<str>);

// Pending children gathered in the first pass, keyed by the parent's arena
// index; columns are then spliced in relation order so each relation's
// columns are contiguous.
struct PendingColumn {
    id: SchemaObjectId,
    name: String,
    data_type: String,
    nullable: bool,
    ordinal: usize,
    default: Option<String>,
}
struct PendingIndex {
    id: SchemaObjectId,
    name: String,
    columns: Vec<String>,
    unique: bool,
    index_type: String,
}

impl CatalogCache {
    /// Build the immutable catalog from a snapshot. Cheap by design: one
    /// allocation per snapshot object plus the maps above.
    pub fn from_snapshot(snapshot: &SchemaSnapshot) -> Self {
        let mut objects: Vec<CatalogEntry> = Vec::with_capacity(snapshot.objects.len());
        let mut by_id = std::collections::HashMap::with_capacity(snapshot.objects.len());
        let mut relations_by_name: std::collections::HashMap<RelKey, u32> = Default::default();
        let mut relations_by_folded_name: std::collections::HashMap<RelKey, Vec<u32>> =
            Default::default();
        // relation id → arena index, for the children passes
        let mut parents: std::collections::HashMap<SchemaObjectId, u32> = Default::default();

        // Pass 1: relations and functions resolve to indices immediately.
        // Snapshot order interleaves the kinds, so they are pooled here and
        // concatenated after: the arena must start with relations in order
        // for the per-relation column ranges below to be contiguous.
        let mut relation_pool: Vec<CatalogEntry> = Vec::new();
        let mut function_pool: Vec<CatalogEntry> = Vec::new();
        for obj in &snapshot.objects {
            match obj {
                SchemaObject::Table {
                    id,
                    schema,
                    name,
                    estimated_rows,
                } => {
                    let key: RelKey = (schema.as_str().into(), name.as_str().into());
                    relations_by_name.insert(key, relation_pool.len() as u32);
                    relations_by_folded_name
                        .entry((fold(schema), fold(name)))
                        .or_default()
                        .push(relation_pool.len() as u32);
                    parents.insert(*id, relation_pool.len() as u32);
                    relation_pool.push(CatalogEntry::Table {
                        id: *id,
                        schema: schema.as_str().into(),
                        name: name.as_str().into(),
                        estimated_rows: *estimated_rows,
                    });
                }
                SchemaObject::View {
                    id,
                    schema,
                    name,
                    definition,
                } => {
                    let key: RelKey = (schema.as_str().into(), name.as_str().into());
                    relations_by_name.insert(key, relation_pool.len() as u32);
                    relations_by_folded_name
                        .entry((fold(schema), fold(name)))
                        .or_default()
                        .push(relation_pool.len() as u32);
                    parents.insert(*id, relation_pool.len() as u32);
                    relation_pool.push(CatalogEntry::View {
                        id: *id,
                        schema: schema.as_str().into(),
                        name: name.as_str().into(),
                        definition: definition.as_str().into(),
                    });
                }
                SchemaObject::Function {
                    id,
                    schema,
                    name,
                    parameters,
                    return_type,
                    language,
                } => {
                    function_pool.push(CatalogEntry::Function {
                        id: *id,
                        schema: schema.as_str().into(),
                        name: name.as_str().into(),
                        parameters: parameters
                            .iter()
                            .map(|(n, t)| (n.as_str().into(), t.as_str().into()))
                            .collect(),
                        return_type: return_type.as_str().into(),
                        language: language.as_str().into(),
                    });
                }
                // The rest is pending until all parents exist.
                _ => {}
            }
        }

        let n_relations = relation_pool.len();
        objects.extend(relation_pool);
        for (slot, entry) in objects.iter().enumerate().take(n_relations) {
            by_id.insert(entry.id(), slot as u32);
        }
        let n_functions = function_pool.len();
        for (entry, slot) in function_pool
            .into_iter()
            .zip((n_relations as u32)..(n_relations as u32 + n_functions as u32))
        {
            by_id.insert(entry.id(), slot);
            objects.push(entry);
        }
        // Pass 2: columns, bucketed per parent relation.
        let mut column_buckets: Vec<Vec<PendingColumn>> =
            (0..n_relations).map(|_| Vec::new()).collect();
        // Pass 3 setup: indexes, bucketed per parent relation.
        let mut index_buckets: Vec<Vec<PendingIndex>> =
            (0..n_relations).map(|_| Vec::new()).collect();
        for obj in &snapshot.objects {
            match obj {
                SchemaObject::Column {
                    id,
                    parent_id,
                    name,
                    data_type,
                    nullable,
                    ordinal,
                    default,
                } => {
                    if let Some(&rel) = parents.get(parent_id) {
                        column_buckets[rel as usize].push(PendingColumn {
                            id: *id,
                            name: name.clone(),
                            data_type: data_type.clone(),
                            nullable: *nullable,
                            ordinal: *ordinal,
                            default: default.clone(),
                        });
                    }
                    // else: parent out of scope — dropped (stage 2 already
                    // guarantees this at the service; the cache must not
                    // panic when a hand-built test snapshot violates it).
                }
                SchemaObject::Index {
                    id,
                    parent_table_id,
                    name,
                    columns,
                    unique,
                    index_type,
                } => {
                    if let Some(&rel) = parents.get(parent_table_id) {
                        index_buckets[rel as usize].push(PendingIndex {
                            id: *id,
                            name: name.clone(),
                            columns: columns.clone(),
                            unique: *unique,
                            index_type: index_type.clone(),
                        });
                    }
                }
                _ => {}
            }
        }

        // Splice columns in relation order, ordinal order within a relation.
        let mut columns_of = vec![Range { start: 0, end: 0 }; n_relations];
        for rel in 0..n_relations {
            let bucket = &mut column_buckets[rel];
            bucket.sort_by_key(|c| c.ordinal);
            let start = objects.len() as u32;
            for c in bucket.drain(..) {
                by_id.insert(c.id, objects.len() as u32);
                objects.push(CatalogEntry::Column {
                    id: c.id,
                    parent: rel as u32,
                    name: c.name.into(),
                    data_type: c.data_type.into(),
                    nullable: c.nullable,
                    ordinal: c.ordinal,
                    default: c.default.map(Box::from),
                });
            }
            columns_of[rel] = start..objects.len() as u32;
        }

        for (rel, bucket) in index_buckets.into_iter().enumerate() {
            for i in bucket {
                by_id.insert(i.id, objects.len() as u32);
                objects.push(CatalogEntry::Index {
                    id: i.id,
                    parent: rel as u32,
                    name: i.name.into(),
                    columns: i.columns.iter().map(|c| c.as_str().into()).collect(),
                    unique: i.unique,
                    index_type: i.index_type.into(),
                });
            }
        }

        let mut schemas: Vec<Box<str>> = objects
            .iter()
            .filter_map(CatalogEntry::schema_owned)
            .collect();
        schemas.sort();
        schemas.dedup();

        let mut functions = 0usize;
        let mut indexes = 0usize;
        let mut columns = 0usize;
        for e in &objects {
            match e {
                CatalogEntry::Function { .. } => functions += 1,
                CatalogEntry::Index { .. } => indexes += 1,
                CatalogEntry::Column { .. } => columns += 1,
                _ => {}
            }
        }

        // Folded (name, idx) sort — deterministic order (name, then arena).
        let mut name_order: Vec<(Box<str>, u32)> = objects
            .iter()
            .enumerate()
            .take(n_relations + n_functions)
            .map(|(idx, e)| (fold(e.name()), idx as u32))
            .collect();
        name_order.sort();

        let stats = CatalogStats {
            relations: n_relations,
            columns,
            indexes,
            functions,
            schemas: schemas.len(),
            keywords: snapshot.keywords.len(),
            snapshot_id: snapshot.id,
            built_at: Instant::now(),
        };

        Self {
            snapshot_id: snapshot.id,
            connection_id: snapshot.connection_id,
            snapshot_version: snapshot.version,
            objects,
            by_id,
            n_relations,
            relations_by_name,
            relations_by_folded_name,
            columns_of,
            name_order,
            schemas,
            keywords: snapshot.keywords.clone(),
            stats,
        }
    }

    /// The snapshot this catalog was built from.
    pub fn snapshot_id(&self) -> SchemaSnapshotId {
        self.snapshot_id
    }

    pub fn version(&self) -> u64 {
        self.snapshot_version
    }

    pub fn connection_id(&self) -> ConnectionId {
        self.connection_id
    }

    /// Total arena length — one slot per snapshot object.
    pub fn len(&self) -> usize {
        self.objects.len()
    }

    pub fn is_empty(&self) -> bool {
        self.objects.is_empty()
    }

    pub fn keywords(&self) -> &[String] {
        &self.keywords
    }

    pub fn schemas(&self) -> &[Box<str>] {
        &self.schemas
    }

    pub fn stats(&self) -> &CatalogStats {
        &self.stats
    }

    pub fn object(&self, idx: u32) -> Option<&CatalogEntry> {
        self.objects.get(idx as usize)
    }

    pub fn by_id(&self, id: SchemaObjectId) -> Option<u32> {
        self.by_id.get(&id).copied()
    }

    /// Quoted-identifier path: exact, case-sensitive `(schema, name)`.
    pub fn relation_exact(&self, schema: &str, name: &str) -> Option<u32> {
        self.relations_by_name
            .get(&((*schema).into(), (*name).into()))
            .copied()
    }

    /// Unquoted-identifier path: folded lookup. An empty vector means no
    /// such relation; more than one means the folded name is ambiguous
    /// (e.g. `users` and quoted `"Users"` coexist) — reported by the
    /// analyzer, never guessed here.
    pub fn relation_folded(&self, schema: &str, name: &str) -> Vec<u32> {
        self.relations_by_folded_name
            .get(&(fold(schema), fold(name)))
            .cloned()
            .unwrap_or_default()
    }

    /// Every relation in one schema, folded compare, sorted by (name, arena
    /// index) so schema-dot completion is deterministic. Empty for a schema
    /// the catalog does not cover (spec §4.4: off-`search_path` schemas are
    /// never introspected by default).
    pub fn relations_in_schema(&self, schema: &str) -> Vec<u32> {
        let folded = fold(schema);
        let mut found: Vec<(Box<str>, u32)> = self
            .relations_by_folded_name
            .iter()
            .filter(|((s, _), _)| *s == folded)
            .flat_map(|((_, name), indices)| indices.iter().map(move |idx| (name.clone(), *idx)))
            .collect();
        found.sort();
        found.into_iter().map(|(_, idx)| idx).collect()
    }

    /// The contiguous, ordinal-ordered column slice of one relation. Empty
    /// for a relation with no introspected columns or for a function index.
    pub fn columns_of(&self, rel_idx: u32) -> &[CatalogEntry] {
        match self.columns_of.get(rel_idx as usize) {
            Some(range) if !range.is_empty() => {
                &self.objects[range.start as usize..range.end as usize]
            }
            _ => &[],
        }
    }

    /// Quoted column reference against one relation.
    pub fn column_exact(&self, rel_idx: u32, name: &str) -> Option<u32> {
        let start = self.columns_of.get(rel_idx as usize)?.start;
        self.columns_of(rel_idx)
            .iter()
            .position(|c| c.name() == name)
            .map(|p| start + p as u32)
    }

    /// Unquoted column reference against one relation: folded compare.
    pub fn column_folded(&self, rel_idx: u32, name: &str) -> Vec<u32> {
        let folded = fold(name);
        let start = self.columns_of.get(rel_idx as usize).map_or(0, |r| r.start);
        self.columns_of(rel_idx)
            .iter()
            .enumerate()
            .filter(|(_, c)| fold(c.name()) == folded)
            .map(|(p, _)| start + p as u32)
            .collect()
    }

    pub fn relation_count(&self) -> usize {
        self.n_relations
    }

    /// Case-folded prefix search over relation and function names, sorted by
    /// (folded name, arena index) so output is deterministic. Returns up to
    /// `cap` indices plus whether more candidates exist beyond the cap —
    /// completion keeps a bounded top-N heap on its side. An empty prefix
    /// matches everything (still capped).
    pub fn prefix_search(&self, prefix: &str, cap: usize) -> (Vec<u32>, bool) {
        let folded = fold(prefix);
        let start = self
            .name_order
            .partition_point(|(name, _)| name.as_ref() < folded.as_ref());
        let mut out = Vec::new();
        for (name, idx) in &self.name_order[start..] {
            if !name.starts_with(&*folded) {
                break;
            }
            if out.len() == cap {
                return (out, true);
            }
            out.push(*idx);
        }
        (out, false)
    }
}

impl CatalogEntry {
    fn schema_owned(&self) -> Option<Box<str>> {
        match self {
            CatalogEntry::Table { schema, .. }
            | CatalogEntry::View { schema, .. }
            | CatalogEntry::Function { schema, .. } => Some(schema.clone()),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests;
