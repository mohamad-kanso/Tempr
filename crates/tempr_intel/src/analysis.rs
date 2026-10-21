//! The analysis model the semantic engine produces per buffer (spec §5.1):
//! per-statement scope trees with resolved bindings, plus diagnostics.
//! Pure data — no catalog access and no tree-sitter types so the model can
//! be unit-tested and consumed by later stages (completion, hover, the UI)
//! without a re-walk.

use std::ops::Range;

/// How bad a diagnostic is. `AmbiguousColumn` is a warning (spec decision 8):
/// an ambiguous reference is *valid SQL* the server may well accept — it
/// deserves attention, not a red line. Everything else is an error.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Severity {
    Warning,
    Error,
}

/// What kind of diagnostic, and with it the resolution stage that produced it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DiagnosticKind {
    /// Lifted from tree-sitter `ERROR` fragments; the grammar cannot parse
    /// this region.
    SyntaxError,
    /// A qualifier (alias, relation name or schema) resolved to nothing.
    UnknownRelation,
    /// A column reference resolved against every visible binding and missed —
    /// only emitted when a catalog is loaded (the silence rule).
    UnknownColumn,
    /// A column reference matched more than one visible binding.
    AmbiguousColumn,
}

impl DiagnosticKind {
    /// The severity this kind arrives with unless something overrides it;
    /// exactly one kind differs from the rest, on purpose (spec decision 8).
    pub fn default_severity(self) -> Severity {
        match self {
            DiagnosticKind::AmbiguousColumn => Severity::Warning,
            _ => Severity::Error,
        }
    }

    /// Errors sort before warnings within `NextDiagnostic` navigation
    /// (stage 6); the tie-break inside a severity is byte order.
    pub fn sort_key(self) -> (u8, u8) {
        let sev = match self.default_severity() {
            Severity::Warning => 1u8,
            Severity::Error => 0u8,
        };
        let kind = match self {
            DiagnosticKind::SyntaxError => 0,
            DiagnosticKind::UnknownRelation => 1,
            DiagnosticKind::UnknownColumn => 2,
            DiagnosticKind::AmbiguousColumn => 3,
        };
        (sev, kind)
    }
}

/// One reported problem with its byte range and human message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub range: Range<usize>,
    pub kind: DiagnosticKind,
    pub severity: Severity,
    pub message: String,
}

impl Diagnostic {
    pub fn new(kind: DiagnosticKind, range: Range<usize>, message: String) -> Self {
        Diagnostic {
            kind,
            severity: kind.default_severity(),
            range,
            message,
        }
    }
}

/// One column that a binding exposes. `Catalog` entries carry the arena
/// index from the catalog cache (type/nullability live there); `Local`
/// columns are names derived from a CTE's or FROM-subquery's select list —
/// no arena equivalent exists.
#[derive(Debug, Clone, PartialEq)]
pub enum ColumnRef {
    Catalog {
        index: u32,
    },
    Local {
        name: Box<str>,
        data_type: Option<Box<str>>,
    },
}

impl ColumnRef {
    /// The display name when it is available locally (Local variants).
    /// Catalog entries get their name from the arena by the caller.
    pub fn local_name(&self) -> Option<&str> {
        match self {
            ColumnRef::Catalog { .. } => None,
            ColumnRef::Local { name, .. } => Some(name),
        }
    }
}

/// One FROM/JOIN/UPDATE/DELETE source visible to a scope: a catalog
/// relation (perhaps aliased), a CTE, or a FROM-subquery.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Binding {
    /// The `AS a` (or bare trailing) alias, if the query stated one.
    pub alias: Option<Box<str>>,
    /// The source's own name: the relation/CTE name (folded lookups win;
    /// stored as reported by the server).
    pub name: Option<Box<str>>,
    /// The schema of a catalog relation, when the query or the catalog
    /// provided one.
    pub schema: Option<Box<str>>,
    /// The arena index of the catalog relation (`relation_exact`/folded
    /// resolution through the catalog cache), `None` for CTEs and
    /// FROM-subqueries.
    pub object: Option<u32>,
    /// The column list the binding exposes, in server/query order.
    pub columns: Vec<ColumnRef>,
    /// Byte range of the source item (hover target, completion anchor).
    pub range: Range<usize>,
}

impl Binding {
    /// Does this binding answer a qualifier? Aliases, the source's own name
    /// and — for schema-qualified use — the schema all fold for the compare
    /// (unquoted SQL identifiers are case-insensitive).
    pub fn matches_qualifier(&self, qualifier: &str) -> bool {
        let folded = qualifier.to_lowercase();
        let hit = |s: Option<&Box<str>>| s.is_some_and(|s| s.to_lowercase() == folded);
        hit(self.alias.as_ref()) || hit(self.name.as_ref()) || hit(self.schema.as_ref())
    }

    /// Positions of columns whose folded names match `name`. Catalog entries
    /// fetch their name through the arena; Local entries carry it. Returns
    /// the matching `ColumnRef`s (folded compare on both sides — unquoted
    /// SQL identifiers are case-insensitive; the quoted-identifier nuance is
    /// parked for stage 5, see TODO).
    pub fn column_hits(
        &self,
        name: &str,
        catalog: &crate::catalog::CatalogCache,
    ) -> Vec<ColumnRef> {
        let folded = name.to_lowercase();
        self.columns
            .iter()
            .filter(|c| match c {
                ColumnRef::Local { name: cn, .. } => cn.to_lowercase() == folded,
                ColumnRef::Catalog { index } => catalog
                    .object(*index)
                    .is_some_and(|e| e.name().to_lowercase() == folded),
            })
            .cloned()
            .collect()
    }
}

/// One scope: the set of sources visible to a region of the query (a
/// statement, or a subquery) plus any child scopes. Visibility inside one
/// scope is `bindings` order; child scopes additionally see their
/// ancestors' bindings (correlated references) and every CTE declared
/// before the region they appear in.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Scope {
    pub range: Range<usize>,
    /// CTEs registered by this region's `WITH` clause, in declaration
    /// order. A CTE shadows a catalog relation of the same folded name when
    /// used as a qualifier and is not referenced by bare-column resolution
    /// unless its columns are themselves bound into `bindings` from its
    /// body's `FROM`.
    pub ctes: Vec<Binding>,
    pub bindings: Vec<Binding>,
    pub children: Vec<Scope>,
}

impl Scope {
    /// Find a source by qualifier across this scope's CTEs (shadowing first)
    /// and bindings.
    pub fn qualifier_binding(&self, qualifier: &str) -> Option<&Binding> {
        self.ctes
            .iter()
            .chain(self.bindings.iter())
            .find(|b| b.matches_qualifier(qualifier))
    }
}

impl Scope {
    /// All visible bindings, own first then ancestors, in scope order.
    pub fn visible<'a>(&'a self, ancestors: &[&'a Scope]) -> Vec<&'a Binding> {
        self.bindings
            .iter()
            .chain(ancestors.iter().flat_map(|s| s.bindings.iter()))
            .collect()
    }
}

/// One buffer's analysis: one top-level scope per statement (subscopes
/// nested), diagnostics in byte order.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Analysis {
    /// Top-level statement scopes, in byte order.
    pub scopes: Vec<Scope>,
    /// Sorted by (byte order, errors before warnings) at construction time
    /// by the engine.
    pub diagnostics: Vec<Diagnostic>,
}

impl Analysis {
    /// Diagnostics overlapping `query`, already severity-ordered.
    pub fn diagnostics_in(&self, query: Range<usize>) -> Vec<&Diagnostic> {
        self.diagnostics
            .iter()
            .filter(|d| d.range.start < query.end && query.start < d.range.end.max(query.end))
            .collect()
    }

    /// Errors before warnings within the query range's frames; stable on ties.
    pub fn next_diagnostic(&self, from: usize) -> Option<&Diagnostic> {
        let mut ordered: Vec<&Diagnostic> = self
            .diagnostics
            .iter()
            .filter(|d| d.range.start >= from)
            .collect();
        ordered.sort_by_key(|d| (d.range.start, d.kind.sort_key()));
        ordered.first().copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::CatalogCache;
    use tempr_domain::ids::{SchemaObjectId, SchemaSnapshotId};
    use tempr_domain::schema::{SchemaObject, SchemaSnapshot};

    fn catalog_with_table(columns: &[&str]) -> CatalogCache {
        let table_id = SchemaObjectId::new();
        let mut objects = vec![SchemaObject::Table {
            id: table_id,
            schema: "public".into(),
            name: "t".into(),
            estimated_rows: None,
        }];
        for (i, name) in columns.iter().enumerate() {
            objects.push(SchemaObject::Column {
                id: SchemaObjectId::new(),
                parent_id: table_id,
                name: (*name).into(),
                data_type: "int8".into(),
                nullable: false,
                ordinal: i,
                default: None,
            });
        }
        CatalogCache::from_snapshot(&SchemaSnapshot {
            id: SchemaSnapshotId::new(),
            connection_id: tempr_domain::ids::ConnectionId::new(),
            version: 1,
            fetched_at: chrono::Utc::now(),
            objects,
            keywords: vec![],
            fingerprints: vec![],
        })
    }

    fn binding(alias: &str, columns: &[&str]) -> Binding {
        Binding {
            alias: Some(alias.into()),
            columns: columns
                .iter()
                .map(|n| ColumnRef::Local {
                    name: (*n).into(),
                    data_type: None,
                })
                .collect(),
            ..Binding::default()
        }
    }

    #[test]
    fn ambiguous_column_is_the_only_warning() {
        for kind in [
            DiagnosticKind::SyntaxError,
            DiagnosticKind::UnknownRelation,
            DiagnosticKind::UnknownColumn,
        ] {
            assert_eq!(kind.default_severity(), Severity::Error);
        }
        assert_eq!(
            DiagnosticKind::AmbiguousColumn.default_severity(),
            Severity::Warning
        );
    }

    #[test]
    fn qualifier_matches_alias_name_or_schema_folded() {
        let bind = binding("U", &["id"]); // stored as whatever the query wrote
        // Unquoted SQL folds case-insensitively.
        assert!(bind.matches_qualifier("u"));
        assert!(bind.matches_qualifier("U"));
        assert!(!bind.matches_qualifier("o"));
    }

    #[test]
    fn column_hits_match_folded_across_local_and_catalog_columns() {
        let catalog = catalog_with_table(&["Id", "Name"]);

        let local_only = binding("t", &["Age"]);
        assert_eq!(
            local_only.column_hits("AGE", &catalog).len(),
            1,
            "`AGE` folds to match the Local column `Age`"
        );
        assert!(
            local_only.column_hits("salary", &catalog).is_empty(),
            "no folded hit without a matching column"
        );

        // A catalog-backed binding resolves the same folded name through
        // the arena rather than carried text.
        let rel = catalog.relation_exact("public", "t").expect("table");
        let col = catalog.columns_of(rel).first().expect("column");
        let catalog_backed = Binding {
            alias: Some("t".into()),
            columns: vec![ColumnRef::Catalog {
                index: catalog.by_id(col.id()).expect("arena index"),
            }],
            ..Binding::default()
        };
        assert_eq!(
            catalog_backed.column_hits("id", &catalog).len(),
            1,
            "catalog columns are folded-matchable through the arena"
        );
        assert!(
            catalog_backed.column_hits("name", &catalog).is_empty(),
            "only the first column is exposed by this binding"
        );
    }

    #[test]
    fn next_diagnostic_puts_errors_before_warnings() {
        let analysis = Analysis {
            scopes: vec![],
            diagnostics: vec![
                Diagnostic::new(DiagnosticKind::AmbiguousColumn, 0..2, "ambiguous".into()),
                Diagnostic::new(DiagnosticKind::UnknownColumn, 0..2, "unknown".into()),
                Diagnostic::new(
                    DiagnosticKind::AmbiguousColumn,
                    4..6,
                    "later warning".into(),
                ),
            ],
        };
        let next = analysis.next_diagnostic(0).expect("diagnostics exist");
        assert_eq!(next.message, "unknown", "error beats earlier warning");
        assert_eq!(next.kind, DiagnosticKind::UnknownColumn);
    }

    #[test]
    fn diagnostics_in_is_byte_ordered() {
        let analysis = Analysis {
            scopes: vec![],
            diagnostics: vec![
                Diagnostic::new(DiagnosticKind::UnknownColumn, 10..12, "one".into()),
                Diagnostic::new(DiagnosticKind::UnknownColumn, 50..52, "two".into()),
            ],
        };
        let hits = analysis.diagnostics_in(11..60);
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].message, "one");
    }
}
