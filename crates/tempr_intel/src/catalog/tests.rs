use tempr_domain::ids::{ConnectionId, SchemaObjectId, SchemaSnapshotId};
use tempr_domain::schema::{SchemaObject, SchemaSnapshot};

use super::CatalogCache;

pub(crate) fn snapshot(objects: Vec<SchemaObject>) -> SchemaSnapshot {
    SchemaSnapshot {
        id: SchemaSnapshotId::new(),
        connection_id: ConnectionId::new(),
        version: 1,
        fetched_at: chrono::Utc::now(),
        objects,
        keywords: vec!["select".to_string(), "from".to_string(), "join".to_string()],
        fingerprints: Vec::new(),
    }
}

pub(crate) fn simple_snapshot() -> SchemaSnapshot {
    let users_table_id = SchemaObjectId::new();
    let orders_id = SchemaObjectId::new();
    let columns = vec![
        SchemaObject::Column {
            id: SchemaObjectId::new(),
            parent_id: users_table_id,
            name: "id".to_string(),
            data_type: "int8".to_string(),
            nullable: false,
            ordinal: 1,
            default: None,
        },
        SchemaObject::Column {
            id: SchemaObjectId::new(),
            parent_id: users_table_id,
            name: "Email".to_string(),
            data_type: "text".to_string(),
            nullable: true,
            ordinal: 0,
            default: None,
        },
    ];
    let quote_users_id = SchemaObjectId::new();
    snapshot(vec![
        SchemaObject::Table {
            id: users_table_id,
            schema: "public".to_string(),
            name: "users".to_string(),
            estimated_rows: Some(120),
        },
        SchemaObject::View {
            id: orders_id,
            schema: "public".to_string(),
            name: "user_views".to_string(),
            definition: "select 1".to_string(),
        },
        SchemaObject::Index {
            id: SchemaObjectId::new(),
            parent_table_id: users_table_id,
            name: "users_pkey".to_string(),
            columns: vec!["id".to_string()],
            unique: true,
            index_type: "btree".to_string(),
        },
        SchemaObject::Function {
            id: SchemaObjectId::new(),
            schema: "public".to_string(),
            name: "total_sales".to_string(),
            parameters: vec![("region".to_string(), "text".to_string())],
            return_type: "numeric".to_string(),
            language: "sql".to_string(),
        },
        SchemaObject::Table {
            id: quote_users_id,
            schema: "public".to_string(),
            name: "Users".to_string(),
            estimated_rows: None,
        },
    ])
    .with_added_columns(columns)
}

// helper adding columns after construction for readability
trait WithColumns {
    fn with_added_columns(self, columns: Vec<SchemaObject>) -> SchemaSnapshot;
}
impl WithColumns for SchemaSnapshot {
    fn with_added_columns(mut self, columns: Vec<SchemaObject>) -> SchemaSnapshot {
        self.objects.extend(columns);
        self
    }
}

#[test]
fn builds_a_cache_from_a_snapshot_of_every_kind() {
    let snap = simple_snapshot();
    let cache = CatalogCache::from_snapshot(&snap);

    let stats = cache.stats();
    assert_eq!(cache.snapshot_id(), snap.id);
    assert_eq!(cache.version(), snap.version);
    assert_eq!(cache.connection_id(), snap.connection_id);
    assert_eq!(cache.len(), snap.objects.len());
    assert_eq!(stats.snapshot_id, snap.id);
    assert_eq!(stats.relations, 3, "users, quoted Users, user_views");
    assert_eq!(stats.columns, 2);
    assert_eq!(stats.indexes, 1);
    assert_eq!(stats.functions, 1);
    assert_eq!(stats.schemas, 1);
    assert_eq!(stats.keywords, 3);
    assert_eq!(cache.keywords(), ["select", "from", "join"]);
    assert_eq!(
        cache.schemas().iter().map(Box::as_ref).collect::<Vec<_>>(),
        vec!["public"]
    );
}

#[test]
fn by_id_resolves_every_arena_entry() {
    let snap = simple_snapshot();
    let cache = CatalogCache::from_snapshot(&snap);

    assert_eq!(cache.len(), 7, "arena holds one slot per snapshot object");
    for obj in &snap.objects {
        let idx = cache
            .by_id(obj.id())
            .unwrap_or_else(|| panic!("id of {:?} missing", obj.kind()));
        let entry = cache.object(idx).expect("arena index valid");
        assert_eq!(entry.id(), obj.id());
        assert_eq!(entry.kind(), obj.kind());
        assert_eq!(
            entry.name(),
            names_from(obj),
            "arena preserves server names"
        );
    }

    assert!(cache.by_id(SchemaObjectId::new()).is_none(), "unknown id");
}

fn names_from(obj: &SchemaObject) -> &str {
    match obj {
        SchemaObject::Table { name, .. }
        | SchemaObject::View { name, .. }
        | SchemaObject::Column { name, .. }
        | SchemaObject::Index { name, .. }
        | SchemaObject::Function { name, .. } => name,
    }
}

// Task 2/3 tests follow in the same order as the implementation plan.

#[test]
fn empty_snapshot_builds_an_empty_cache() {
    let snap = snapshot(vec![]);
    let cache = CatalogCache::from_snapshot(&snap);
    assert_eq!(cache.len(), 0);
    assert!(cache.object(0).is_none());
    assert_eq!(cache.stats().relations, 0);
    assert_eq!(cache.prefix_search("us", 10).0, Vec::<u32>::new());
}

#[test]
fn duplicate_ids_keep_the_last_object() {
    let shared = SchemaObjectId::new();
    let snap = snapshot(vec![
        SchemaObject::Table {
            id: shared,
            schema: "public".to_string(),
            name: "first".to_string(),
            estimated_rows: None,
        },
        SchemaObject::Table {
            id: shared,
            schema: "public".to_string(),
            name: "second".to_string(),
            estimated_rows: None,
        },
    ]);
    let cache = CatalogCache::from_snapshot(&snap);
    let idx = cache.by_id(shared).expect("duplicate id resolves");
    assert_eq!(cache.object(idx).expect("entry").name(), "second");
}

#[test]
fn a_column_whose_parent_is_missing_is_dropped() {
    let snap = snapshot(vec![SchemaObject::Column {
        id: SchemaObjectId::new(),
        parent_id: SchemaObjectId::new(),
        name: "orphan".to_string(),
        data_type: "int4".to_string(),
        nullable: false,
        ordinal: 0,
        default: None,
    }]);
    let cache = CatalogCache::from_snapshot(&snap);
    assert_eq!(cache.stats().columns, 0, "orphan column not cached");
    assert_eq!(cache.len(), 0);
}

// The entries carry parent arena indices.

#[test]
fn columns_and_indexes_reference_their_relation() {
    let snap = simple_snapshot();
    let cache = CatalogCache::from_snapshot(&snap);

    let users_id = snap
        .objects
        .iter()
        .find_map(|o| match o {
            SchemaObject::Table { id, name, .. } if name == "users" => Some(*id),
            _ => None,
        })
        .expect("users");

    let users_idx = cache.by_id(users_id).expect("relation present");
    for entry in cache.columns_of(users_idx) {
        assert_eq!(entry.parent(), Some(users_idx));
    }
}

// === Task 2: relation, schema and column lookups ===

#[test]
fn quoted_relations_match_exactly_and_unquoted_fold() {
    let snap = simple_snapshot();
    let cache = CatalogCache::from_snapshot(&snap);

    // Exact path: the two coexisting relations are distinct entries.
    let users = cache.relation_exact("public", "users").expect("users");
    let quoted_users = cache
        .relation_exact("public", "Users")
        .expect("quoted Users");
    assert_ne!(users, quoted_users);

    // Wrong case never matches the exact map.
    assert!(cache.relation_exact("public", "USERS").is_none());

    // Folded path: the unquoted `USERS` in a query refers to *both* — the
    // catalog reports the candidates; the analyzer decides (ambiguity).
    let folded = cache.relation_folded("PUBLIC", "uSeRs");
    assert_eq!(folded.len(), 2, "{folded:?}");
    let mut sorted = folded.clone();
    sorted.sort();
    assert_eq!(folded, sorted, "candidates are in arena order");

    // Nothing in the other direction or unknown schema.
    assert!(cache.relation_folded("private", "users").is_empty());
    assert!(cache.relation_folded("public", "orders").is_empty());
}

#[test]
fn columns_are_a_contiguous_ordinal_ordered_slice() {
    let snap = simple_snapshot();
    let cache = CatalogCache::from_snapshot(&snap);

    let users_idx = cache.relation_exact("public", "users").expect("users");
    let columns = cache.columns_of(users_idx);
    assert_eq!(columns.len(), 2);
    let ordinals: Vec<usize> = columns
        .iter()
        .map(|c| match c {
            super::CatalogEntry::Column { ordinal, .. } => *ordinal,
            other => panic!("unexpected entry: {:?}", other.kind()),
        })
        .collect();
    assert_eq!(
        ordinals,
        vec![0, 1],
        "id(Email, 0), id(id, 1): sorted by ordinal"
    );

    // The slice is contiguous: second column follows the first.
    for pair in columns.windows(2) {
        let a = cache.by_id(pair[0].id()).expect("first");
        let b = cache.by_id(pair[1].id()).expect("second");
        assert_eq!(b, a + 1, "columns must be adjacent in the arena");
    }

    // Quoted vs folded column lookup.
    assert_eq!(
        cache
            .column_exact(users_idx, "Email")
            .expect("exact column"),
        cache.by_id(columns[0].id()).expect("arena idx")
    );
    assert!(
        cache.column_exact(users_idx, "email").is_none(),
        "exact is case-sensitive"
    );
    assert_eq!(
        cache.column_folded(users_idx, "EMAIL"),
        vec![cache.by_id(columns[0].id()).expect("arena idx")]
    );
    assert!(cache.column_folded(users_idx, "missing").is_empty());

    // A function index has no columns.
    let view_idx = cache.relation_exact("public", "user_views").expect("view");
    assert!(cache.columns_of(view_idx).is_empty());
}

// === Task 3: prefix search ===

#[test]
fn prefix_search_is_folded_and_deterministic() {
    let snap = simple_snapshot();
    let cache = CatalogCache::from_snapshot(&snap);

    // "total_sales" (function) and "users"/"Users"/"user_views". Folded
    // names sort lexicographically: "user_views" < "users" ('_' < 's'), and
    // the two folded-"users" relations tie in arena order.
    let (hits, more) = cache.prefix_search("us", 10);
    assert!(!more);
    let names: Vec<&str> = hits
        .iter()
        .map(|i| cache.object(*i).expect("entry").name())
        .collect();
    assert_eq!(names, vec!["user_views", "users", "Users"]);
}

#[test]
fn prefix_search_respects_the_cap_and_reports_more() {
    let snap = simple_snapshot();
    let cache = CatalogCache::from_snapshot(&snap);

    let (hits, more) = cache.prefix_search("u", 2);
    assert_eq!(hits.len(), 2);
    assert!(more, "unmatched candidates exist beyond the cap");

    let (all, all_more) = cache.prefix_search("u", 10);
    assert!(all.len() >= 2);
    assert!(!all_more);

    assert!(cache.prefix_search("zzz_no_match", 10).0.is_empty());
}

#[test]
fn two_caches_built_from_the_same_snapshot_agree() {
    let snap = simple_snapshot();
    let a = CatalogCache::from_snapshot(&snap);
    let b = CatalogCache::from_snapshot(&snap);
    assert_eq!(a.prefix_search("u", 10), b.prefix_search("u", 10));
}

#[test]
fn empty_catalog_prefix_search_is_empty() {
    let snap = snapshot(vec![]);
    let cache = CatalogCache::from_snapshot(&snap);
    let (hits, more) = cache.prefix_search("", 10);
    assert!(hits.is_empty() && !more);
}
