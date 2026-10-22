//! Release-only timing probes (spec §8 "Performance", in the style of
//! `tempr_editor`'s D21/D22 checks). Ignored by default; run with:
//!
//! ```sh
//! cargo test --release -p tempr_intel -- --ignored --nocapture
//! ```
//!
//! The synthetic catalog carries ~10 000 objects (1 200 relations × 8
//! columns, 1 200 indexes, 800 functions plus keywords), sized like the
//! 10 000-object ceiling the completion budget (< 5 ms, stage 5) is written
//! against. Numbers are p50/p95/p99/max over 40 runs of:
//!
//! 1. `from_snapshot` — the catalog build a refresh pays once;
//! 2. hot-path mix — `prefix_search` + `relation_folded` + `columns_of` +
//!    `column_folded`, the request-shape lookups completion will do;
//! 3. `.tcat` load — decode + build from in-memory bytes (no disk I/O, so
//!    the number isolates the codec + the build; the OD#1/D27 evidence).
//!
//! The probe asserts code sanity only (candidate counts), never timing.

use tempr_domain::ids::{ConnectionId, SchemaObjectId, SchemaSnapshotId};
use tempr_domain::schema::{SchemaObject, SchemaObjectKind, SchemaSnapshot};

use super::CatalogCache;
use super::completion::CompletionRequest;
use super::semantic::SemanticEngine;

const RELATIONS: usize = 1200;
const COLUMNS_PER_RELATION: usize = 8;
const FUNCTIONS: usize = 800;
const KEYWORDS: usize = 50;
const RUNS: u32 = 40;

fn synthetic_snapshot() -> SchemaSnapshot {
    let connection = ConnectionId::new();

    let mut objects: Vec<SchemaObject> =
        Vec::with_capacity(RELATIONS * (2 + COLUMNS_PER_RELATION) + FUNCTIONS);

    for n in 0..RELATIONS as u64 {
        let table_id = SchemaObjectId::derived(
            connection,
            SchemaObjectKind::Table,
            n, // distinct from the column packing below within its kind
        );
        objects.push(SchemaObject::Table {
            id: table_id,
            schema: "public".to_string(),
            name: format!("rel_{n:06}"),
            estimated_rows: Some(10_000),
        });
        for c in 0..COLUMNS_PER_RELATION as u64 {
            objects.push(SchemaObject::Column {
                id: SchemaObjectId::derived(connection, SchemaObjectKind::Column, (n << 16) | c),
                parent_id: table_id,
                name: format!("col_{c:02}"),
                data_type: "int8".to_string(),
                nullable: c % 2 == 0,
                ordinal: c as usize,
                default: None,
            });
        }
        let index_id = SchemaObjectId::derived(
            connection,
            SchemaObjectKind::Index,
            n, // distinct from the column packing below within its kind
        );
        objects.push(SchemaObject::Index {
            id: index_id,
            parent_table_id: table_id,
            name: format!("rel_{n:06}_pkey"),
            columns: vec!["col_00".to_string()],
            unique: true,
            index_type: "btree".to_string(),
        });
    }

    for n in 0..FUNCTIONS {
        objects.push(SchemaObject::Function {
            id: SchemaObjectId::new(),
            schema: "public".to_string(),
            name: format!("func_{n:06}"),
            parameters: vec![],
            return_type: "text".to_string(),
            language: "sql".to_string(),
        });
    }

    SchemaSnapshot {
        id: SchemaSnapshotId::new(),
        connection_id: connection,
        version: 1,
        fetched_at: chrono::Utc::now(),
        objects,
        keywords: (0..KEYWORDS).map(|n| format!("kw_{n:02}")).collect(),
        fingerprints: Vec::new(),
    }
}

fn percentiles(
    mut times: Vec<std::time::Duration>,
) -> (
    std::time::Duration,
    std::time::Duration,
    std::time::Duration,
    std::time::Duration,
) {
    times.sort_unstable();
    let pick = |q: f64| times[(q * (times.len() as f64 - 1.0)).round() as usize];
    (
        times[times.len() / 2],
        pick(0.95),
        pick(0.99),
        *times.last().expect("at least one run"),
    )
}

fn ms(d: std::time::Duration) -> String {
    format!("{:.3} ms", d.as_secs_f64() * 1e3)
}

fn report(label: &str, times: Vec<std::time::Duration>) {
    let (p50, p95, p99, max) = percentiles(times);
    println!(
        "{label}: p50 {} · p95 {} · p99 {} · max {}",
        ms(p50),
        ms(p95),
        ms(p99),
        ms(max)
    );
}

#[test]
#[ignore = "release-only timing probe"]
fn probe_catalog_build_hot_path_and_tcat_load() {
    let snapshot = synthetic_snapshot();

    // === Leg 1: build from snapshot ===
    let mut build_times = Vec::new();
    for _ in 0..RUNS {
        let start = std::time::Instant::now();
        let cache = CatalogCache::from_snapshot(&snapshot);
        build_times.push(start.elapsed());
        assert_eq!(
            cache.len(),
            RELATIONS + RELATIONS * COLUMNS_PER_RELATION + RELATIONS + FUNCTIONS,
            "every object lands in the arena",
        );
        assert_eq!(cache.stats().relations, RELATIONS);
    }
    report(
        &format!(
            "build from_snapshot ({RELATIONS} relations × {COLUMNS_PER_RELATION} cols + indexes + {FUNCTIONS} functions)"
        ),
        build_times,
    );

    // === Leg 2: hot-path request mix ===
    let cache = CatalogCache::from_snapshot(&snapshot);
    let some_table = cache.relation_exact("public", "rel_000999").expect("table");
    assert_eq!(
        cache.relation_folded("public", "rel_000999").len(),
        1,
        "exact-fold lookup sanity",
    );
    assert!(
        !cache.columns_of(some_table).is_empty(),
        "column slice sanity"
    );
    let mut mix_times = Vec::new();
    for _ in 0..RUNS {
        let start = std::time::Instant::now();
        let (relations, more) = cache.prefix_search("rel", 200);
        let folded = cache.relation_folded("public", "rel_000999");
        let columns = cache.columns_of(some_table);
        let column = cache.column_folded(some_table, "col_03");
        mix_times.push(start.elapsed());
        assert!(!relations.is_empty() && more, "prefix row set sanity");
        assert_eq!(folded.len(), 1);
        assert_eq!(columns.len(), COLUMNS_PER_RELATION);
        assert_eq!(column.len(), 1);
    }
    report(
        "hot-path mix (prefix_search + relation_folded + columns_of + column_folded)",
        mix_times,
    );

    // === Leg 3: `.tcat` decode + build, in-memory bytes ===
    let bytes = tempr_workspace::encode_catalog(&snapshot).expect("encode sanity");
    let decoded = tempr_workspace::decode_catalog(&bytes)
        .expect("codec never errors on its own output")
        .expect("header must match");
    assert_eq!(decoded.id, snapshot.id);
    let mut tcat_times = Vec::new();
    for _ in 0..RUNS {
        let start = std::time::Instant::now();
        let Ok(Some(decoded)) = tempr_workspace::decode_catalog(&bytes) else {
            panic!("decode must succeed for bytes it just encoded");
        };
        let cache = CatalogCache::from_snapshot(&decoded);
        tcat_times.push(start.elapsed());
        assert_eq!(
            cache.len(),
            RELATIONS + RELATIONS * 8 + RELATIONS + FUNCTIONS
        );
    }
    report(
        &format!(
            "load from .tcat ({:?}, {} bytes) = decode + build",
            snapshot.id,
            bytes.len()
        ),
        tcat_times,
    );
}

/// Completion p99 on the same 10 000-object catalog (spec §8's hard budget).
/// Warm requests: the analysis cache is valid across iterations (buffer
/// revision unchanged), which is the steady state typing produces. The first
/// request per buffer (reparse + analyze) is reported separately as "cold".
#[test]
#[ignore = "release-only timing probe"]
fn probe_completion_latency() {
    use tempr_domain::SqlFileId;
    use tempr_editor::Buffer;

    let snapshot = synthetic_snapshot();
    let engine = SemanticEngine::new(std::sync::Arc::new(CatalogCache::from_snapshot(&snapshot)));

    let cases = [
        (
            "relation prefix (200 of 1 200 relations)",
            "select * from rel_0001",
        ),
        ("expression column prefix", "select col_ from rel_000999"),
        (
            "qualifier dot (columns of one binding)",
            "select * from rel_000999 r where r.",
        ),
        (
            "schema dot (all relations of a schema)",
            "select * from public.",
        ),
        ("ctrl-space, empty relation prefix", "select * from "),
        ("function prefix", "select func_ from "),
    ];

    for (label, sql) in cases {
        let mut buffer = Buffer::new(SqlFileId::new(), sql);
        let file = buffer.file_id();
        let offset = buffer.len();

        // Cold: first request analyzes the buffer.
        let start = std::time::Instant::now();
        let items = engine.complete(CompletionRequest {
            file,
            buffer: &mut buffer,
            offset,
            explicit: true,
        });
        let cold = start.elapsed();
        assert!(!items.is_empty(), "{label}: candidates");

        let mut times = Vec::with_capacity(RUNS as usize);
        for _ in 0..RUNS {
            let start = std::time::Instant::now();
            let items = engine.complete(CompletionRequest {
                file,
                buffer: &mut buffer,
                offset,
                explicit: true,
            });
            times.push(start.elapsed());
            assert!(!items.is_empty(), "{label}: candidates");
        }
        let (p50, p95, p99, max) = percentiles(times);
        println!(
            "{label}: cold {} · warm p50 {} · p95 {} · p99 {} · max {}",
            ms(cold),
            ms(p50),
            ms(p95),
            ms(p99),
            ms(max)
        );
    }

    let mut typing_sql = String::new();
    for n in 0..500 {
        typing_sql.push_str(&format!("select col_00 from rel_{n:06};\n"));
    }
    typing_sql.push_str("select * from rel_0001");
    let mut buffer = Buffer::new(SqlFileId::new(), &typing_sql);
    let file = buffer.file_id();
    let mut times = Vec::with_capacity(RUNS as usize);
    for i in 0..RUNS {
        let offset = buffer.len();
        let digit = if i % 2 == 0 { "2" } else { "1" };
        buffer
            .edit(&[(offset - 1..offset, digit)])
            .expect("typing edit");
        let offset = buffer.len();
        let start = std::time::Instant::now();
        let items = engine.complete(CompletionRequest {
            file,
            buffer: &mut buffer,
            offset,
            explicit: false,
        });
        times.push(start.elapsed());
        assert!(!items.is_empty(), "typing probe: candidates");
    }
    let (p50, p95, p99, max) = percentiles(times);
    println!(
        "per-keystroke completion over 500 statements: p50 {} · p95 {} · p99 {} · max {}",
        ms(p50),
        ms(p95),
        ms(p99),
        ms(max)
    );
}
