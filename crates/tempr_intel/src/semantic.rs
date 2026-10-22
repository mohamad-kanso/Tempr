//! The semantic engine (spec §5.1): walks the editor's tree-sitter SQL tree
//! per statement, builds scope trees (aliases, CTEs, subqueries), resolves
//! column references against bindings and the `CatalogCache`, and emits the
//! diagnostics model. No I/O and no DB queries — ADR-0009.
//!
//! Severity rules (spec decision 8): `AmbiguousColumn` is a warning; the
//! other kinds are errors. Silence rules (§5.1, §7): with no catalog loaded
//! the engine emits no semantic diagnostics at all; ranges covered by
//! grammar `ERROR` fragments produce only syntax diagnostics and suppress
//! semantic ones there.
//!
//! The walk runs in two phases per region so clause order can never fool
//! visibility: phase A collects the region's bindings (relations, lateral
//! joins, `UPDATE`/`DELETE`/`INSERT` targets, `WITH` CTEs with their column
//! lists) without analyzing any subquery body; phase B analyzes every
//! subquery and CTE body *after* the region's frame is complete, so a
//! `select`-list subquery still sees a `FROM` alias appearing later in the
//! source.

use std::collections::HashMap;
use std::ops::Range;
use std::sync::Arc;

use parking_lot::{Mutex, RwLock};
use tempr_domain::SqlFileId;
use tempr_editor::Buffer;
use tree_sitter::Node;

use crate::analysis::{Analysis, Binding, ColumnRef, Diagnostic, DiagnosticKind, Scope, Severity};
use crate::catalog::CatalogCache;
use crate::hover::HoverInfo;

/// Beyond this many cached analyses the engine self-trims by insertion age;
/// the stage-6 service evicts per buffer on close anyway.
const ANALYSIS_CACHE_MAX: usize = 64;

pub struct SemanticEngine {
    catalog: RwLock<Arc<CatalogCache>>,
    analyses: Mutex<HashMap<SqlFileId, CachedAnalysis>>,
}

struct CachedAnalysis {
    revision: u64,
    catalog: Arc<CatalogCache>,
    analysis: Arc<Analysis>,
}

/// Bindings visible at one nesting level: the region's `WITH` CTEs
/// (qualifier-addressable, shadowing relations — never consulted by
/// bare-column resolution unless their columns are themselves bound in
/// through a `FROM` reference) and the region's own relation sources.
#[derive(Debug, Clone, Default)]
struct Frame {
    ctes: Vec<Binding>,
    bindings: Vec<Binding>,
}

impl Frame {
    /// Find a source by folded qualifier: own CTEs shadow bindings.
    fn qualifier(&self, qualifier: &str) -> Option<&Binding> {
        self.ctes
            .iter()
            .find(|b| b.matches_qualifier(qualifier))
            .or_else(|| {
                self.bindings
                    .iter()
                    .find(|b| b.matches_qualifier(qualifier))
            })
    }
}

/// Chain of frames, innermost first: `[own, enclosing…]`.
type Frames = Vec<Frame>;

/// How a (possibly schema-qualified) relation reference resolved.
enum Resolution {
    /// A CTE binding (own region's first, then enclosing frames') cloned.
    Cte(Box<Binding>),
    /// Arena index into the catalog.
    Catalog { idx: u32 },
    /// Nothing to bind: the FROM item carries no column knowledge.
    None,
}
impl SemanticEngine {
    /// An engine over `catalog`; an empty catalog engages the silence rule
    /// (no semantic diagnostics until a refresh or offline load fills it).
    pub fn new(catalog: Arc<CatalogCache>) -> Self {
        Self {
            catalog: RwLock::new(catalog),
            analyses: Mutex::new(HashMap::new()),
        }
    }

    /// Swap the catalog (a refresh built a new one). Analyses may reference
    /// it, so the analysis cache resets.
    pub fn set_catalog(&self, catalog: Arc<CatalogCache>) {
        *self.catalog.write() = catalog;
        self.analyses.lock().clear();
    }

    pub fn catalog(&self) -> Arc<CatalogCache> {
        self.catalog.read().clone()
    }

    /// Analyze a buffer; returns a cached `Arc` while the buffer's text is
    /// unchanged. The key is `Buffer::revision`, which moves on every text
    /// mutation — edits, undo and redo (unlike `EditId`, which undo reuses).
    pub fn analyze(&self, file: SqlFileId, buffer: &mut Buffer) -> Arc<Analysis> {
        self.analyzed(file, buffer.revision(), buffer).0
    }

    /// Analyze `buffer` under the editor's revision, not the buffer's own:
    /// the service's mirror buffer has its own revision counter.
    pub fn analyze_at(
        &self,
        file: SqlFileId,
        source_revision: u64,
        buffer: &mut Buffer,
    ) -> Arc<Analysis> {
        self.analyzed(file, source_revision, buffer).0
    }

    /// Analyze and return the exact catalog the result was built against. A
    /// cache hit is valid only while that catalog is still installed: a
    /// `set_catalog` racing an in-flight analysis can insert an entry built
    /// from the old arena after the map was cleared.
    pub(crate) fn analyzed(
        &self,
        file: SqlFileId,
        revision: u64,
        buffer: &mut Buffer,
    ) -> (Arc<Analysis>, Arc<CatalogCache>) {
        let catalog = self.catalog();
        if let Some(cached) = self.analyses.lock().get(&file)
            && cached.revision == revision
            && Arc::ptr_eq(&cached.catalog, &catalog)
        {
            return (cached.analysis.clone(), catalog);
        }
        let analysis = Arc::new(self.analyze_now(buffer, &catalog));
        let mut cache = self.analyses.lock();
        if cache.len() >= ANALYSIS_CACHE_MAX {
            let oldest = cache.keys().next().copied();
            // Bounded above guarantees non-empty; the option keeps the
            // no-expect rule instead of panicking on an invariant.
            if let Some(oldest) = oldest {
                cache.remove(&oldest);
            }
        }
        cache.insert(
            file,
            CachedAnalysis {
                revision,
                catalog: catalog.clone(),
                analysis: analysis.clone(),
            },
        );
        (analysis, catalog)
    }

    /// Analysis for one offset, skipping statements that cannot contain it.
    /// Completion and hover run after every edit, so the full walk would
    /// scale with file size on the UI thread; a cached full analysis is still
    /// preferred, and its scopes are identical for the requested offset
    /// because `scope_chain` only descends into containing regions.
    pub(crate) fn analyzed_at(
        &self,
        file: SqlFileId,
        revision: u64,
        offset: usize,
        buffer: &mut Buffer,
    ) -> (Arc<Analysis>, Arc<CatalogCache>) {
        let catalog = self.catalog();
        if let Some(cached) = self.analyses.lock().get(&file)
            && cached.revision == revision
            && Arc::ptr_eq(&cached.catalog, &catalog)
        {
            return (cached.analysis.clone(), catalog);
        }
        let analysis = Arc::new(self.analyze_region_now(buffer, &catalog, offset));
        (analysis, catalog)
    }

    /// The hover card for the reference under `offset` (spec §5.3). Reads the
    /// cached analysis completion uses; no I/O and no catalog allocation.
    pub fn hover(&self, file: SqlFileId, buffer: &mut Buffer, offset: usize) -> Option<HoverInfo> {
        let offset = offset.min(buffer.len());
        let text = buffer.text().to_string();
        let reference = crate::hover::reference_at(&text, offset)?;
        let (analysis, catalog) = self.analyzed_at(file, buffer.revision(), offset, buffer);
        let contents = crate::hover::resolve(&catalog, &analysis, offset, &reference)?;
        Some(HoverInfo {
            contents,
            range: reference.range,
        })
    }

    fn analyze_now(&self, buffer: &mut Buffer, catalog: &Arc<CatalogCache>) -> Analysis {
        buffer.syntax(); // lazy reparse when edited
        let fresh = buffer.fresh_syntax();
        let Some(tree) = fresh else {
            return Analysis::default();
        };
        let mut walker = Walker {
            buffer,
            catalog: catalog.as_ref(),
            loaded: catalog.stats().relations > 0,
            diags: Vec::new(),
        };

        let mut scopes = Vec::new();
        for region in regions(tree.root_node()) {
            if region.kind() == "ERROR" {
                // A bare top-level fragment is not a statement: report it and
                // keep it out of the scope list, so `scope_chain` falls back
                // to the statement the fragment trails (completion needs the
                // bindings of a half-typed clause).
                walker.push_syntax_error(region);
                continue;
            }
            scopes.push(walker.walk_region(region, &Vec::new()));
        }

        let mut diagnostics = walker.diags;
        diagnostics.sort_by(|a, b| {
            (a.range.start, a.kind.sort_key()).cmp(&(b.range.start, b.kind.sort_key()))
        });
        Analysis {
            scopes,
            diagnostics,
        }
    }

    fn analyze_region_now(
        &self,
        buffer: &mut Buffer,
        catalog: &Arc<CatalogCache>,
        offset: usize,
    ) -> Analysis {
        buffer.syntax();
        let fresh = buffer.fresh_syntax();
        let Some(tree) = fresh else {
            return Analysis::default();
        };
        let mut walker = Walker {
            buffer,
            catalog: catalog.as_ref(),
            loaded: catalog.stats().relations > 0,
            diags: Vec::new(),
        };
        let regions: Vec<_> = regions(tree.root_node())
            .into_iter()
            .filter(|region| region.kind() != "ERROR")
            .collect();
        // Completion falls back to the statement an incomplete trailing
        // clause leaves behind, so that statement must be present even when
        // the cursor sits outside its byte range.
        let fallback = regions
            .iter()
            .rposition(|region| region.start_byte() <= offset);
        let mut scopes = Vec::new();
        for (index, region) in regions.iter().enumerate() {
            let range = region.byte_range();
            let contains = range.start <= offset && offset <= range.end;
            if contains || fallback == Some(index) {
                scopes.push(walker.walk_region(*region, &Vec::new()));
            }
        }
        Analysis {
            scopes,
            diagnostics: Vec::new(),
        }
    }
}

struct Walker<'a> {
    buffer: &'a Buffer,
    catalog: &'a CatalogCache,
    loaded: bool,
    diags: Vec<Diagnostic>,
}

impl<'a> Walker<'a> {
    fn text(&self, range: Range<usize>) -> String {
        self.buffer.slice(range).unwrap_or_default()
    }

    /// All `identifier` descendants in source order, with byte ranges.
    /// Nested `subquery`/`cte` subtrees are excluded: their identifiers
    /// belong to other scopes.
    fn identifier_parts(&self, node: Node<'a>) -> Vec<(String, Range<usize>)> {
        let mut ranges = Vec::new();
        collect_identifier_ranges(node, &mut ranges);
        ranges
            .into_iter()
            .map(|r| (self.text(r.clone()), r))
            .collect()
    }

    // === Region analysis ===

    /// Analyze one region. A region is a *container* of clause siblings, not
    /// the `select` node: a `statement`/`subquery` holds `select`, `from`,
    /// `where`, … as siblings, so walking only the `select` would lose every
    /// binding and predicate (the grammar shape the stage-4 probe recorded).
    /// `outer` holds the enclosing frames, nearest first.
    fn walk_region(&mut self, region: Node<'a>, outer: &Frames) -> Scope {
        if region.kind() == "ERROR" {
            // A root-level fragment that is nothing but an error.
            self.diags.push(Diagnostic::new(
                DiagnosticKind::SyntaxError,
                region.byte_range(),
                "syntax error".into(),
            ));
            return Scope {
                range: region.byte_range(),
                ..Scope::default()
            };
        }

        // CTEs in declaration order. For a subquery carrying its own `WITH`
        // the `cte` nodes are direct children of the `subquery` node.
        let mut ctes: Vec<Binding> = Vec::new();
        for cte in direct(region, "cte") {
            ctes.push(self.cte_binding(cte, &ctes));
        }

        let nested_setops = direct(region, "set_operation");
        if region.kind() == "set_operation" || !nested_setops.is_empty() {
            return self.walk_set_operation(region, ctes, outer, nested_setops);
        }

        let parts = kids(region);
        self.walk_parts(parts, region.byte_range(), ctes, Some(region), outer)
    }

    /// A set-operation region: the statement itself binds nothing; each
    /// `select` side and its clause siblings form their own scope. Merging
    /// the sides' bindings would invent ambiguity the server does not see.
    fn walk_set_operation(
        &mut self,
        region: Node<'a>,
        ctes: Vec<Binding>,
        outer: &Frames,
        ops: Vec<Node<'a>>,
    ) -> Scope {
        let frame = Frame {
            ctes,
            bindings: Vec::new(),
        };
        let mut full: Frames = vec![frame.clone()];
        full.extend(outer.iter().cloned());
        let mut children = Vec::new();
        // CTE bodies of the statement (only earlier CTEs visible inside).
        self.cte_body_scopes(region, &frame, &full, &mut children);
        let ops = if region.kind() == "set_operation" {
            vec![region]
        } else {
            ops
        };
        for op in ops {
            for side in set_operation_sides(op) {
                let range = parts_range(&side);
                children.push(self.walk_parts(side, range, Vec::new(), None, &full));
            }
        }
        Scope {
            range: region.byte_range(),
            ctes: frame.ctes.clone(),
            bindings: Vec::new(),
            children,
        }
    }

    /// Analyze one region body from its top-level clause nodes (`select`,
    /// `from`, `where`, …). `cte_owner` is the node whose direct `cte`
    /// children need their bodies analyzed; side scopes pass `None`.
    fn walk_parts(
        &mut self,
        parts: Vec<Node<'a>>,
        range: Range<usize>,
        ctes: Vec<Binding>,
        cte_owner: Option<Node<'a>>,
        outer: &Frames,
    ) -> Scope {
        // ---- Phase A: CTEs are already bound; collect relations without
        // analyzing subquery bodies.
        let mut bindings = Vec::new();
        for part in &parts {
            self.phase_a(*part, &ctes, outer, &mut bindings);
        }
        let frame = Frame { ctes, bindings };

        // ---- Phase B: subquery and CTE bodies over the complete frame.
        let mut full: Frames = vec![frame.clone()];
        full.extend(outer.iter().cloned());
        let mut children = Vec::new();
        if let Some(owner) = cte_owner {
            self.cte_body_scopes(owner, &frame, &full, &mut children);
        }
        for part in &parts {
            if part.kind() == "set_operation" {
                // A parenthesized set-operation side: its own scopes.
                children.push(self.walk_region(*part, &full));
            } else {
                self.collect_subqueries(*part, &full, &mut children);
            }
        }
        self.resolve_parts(&parts, &frame, &full);

        Scope {
            range,
            ctes: frame.ctes.clone(),
            bindings: frame.bindings.clone(),
            children,
        }
    }

    /// The CTE bodies of `region`, each a child scope; only the *earlier*
    /// CTEs are visible inside a body (standard `WITH` semantics — and a
    /// `WITH RECURSIVE` self-reference stays silent because its binding
    /// resolves with no column knowledge above it).
    fn cte_body_scopes(
        &mut self,
        region: Node<'a>,
        frame: &Frame,
        full: &Frames,
        out: &mut Vec<Scope>,
    ) {
        for (i, cte) in direct(region, "cte").into_iter().enumerate() {
            if let Some(body) = cte_body(cte) {
                let prefix = Frame {
                    ctes: frame.ctes[..i].to_vec(),
                    bindings: Vec::new(),
                };
                let mut frames = vec![prefix];
                frames.extend(full.iter().skip(1).cloned());
                let mut sub = self.walk_region(body, &frames);
                sub.range = body.byte_range();
                out.push(sub);
            }
        }
    }

    /// Analyze every `subquery` body in the parts' subtrees (source order,
    /// each exactly once; found subqueries are not descended into here — the
    /// body owns its own walk).
    fn collect_subqueries(&mut self, node: Node<'a>, full: &Frames, out: &mut Vec<Scope>) {
        match node.kind() {
            "subquery" => {
                out.push(self.walk_region(node, full));
                return;
            }
            "ERROR" | "cte" => return, // CTE bodies handled in `walk_region`
            _ => {}
        }
        for child in kids(node) {
            self.collect_subqueries(child, full, out);
        }
    }

    // === Phase A ===

    /// Collect the region's source bindings in source order. Never descends
    /// into `subquery` (phase B defers their bodies), CTE bodies, `ERROR`
    /// fragments, or pure expressions.
    fn phase_a(
        &mut self,
        node: Node<'a>,
        ctes: &[Binding],
        outer: &Frames,
        bindings: &mut Vec<Binding>,
    ) {
        match node.kind() {
            "field" | "literal" | "parameter" | "all_fields" | "invocation" | "case"
            | "subquery" | "cte" | "ERROR" | "set_operation" => {}
            k if k.starts_with("keyword_") => {}
            "relation" => self.relation_item(node, ctes, outer, bindings),
            "lateral_join" | "lateral_cross_join" => self.lateral_item(node, bindings),
            "from" => {
                // `DELETE FROM t` keeps the target in the statement-level
                // `from` sibling, not under `delete` (grammar shape).
                if is_delete_target_clause(node) {
                    self.dml_from(node, ctes, outer, bindings);
                }
                for child in kids(node) {
                    self.phase_a(child, ctes, outer, bindings);
                }
            }
            "insert" => {
                self.dml_target(node, ctes, outer, bindings);
                // Keep descending: the `INSERT … SELECT … FROM` source may
                // bind more relations.
                for child in kids(node) {
                    self.phase_a(child, ctes, outer, bindings);
                }
            }
            _ => {
                for child in kids(node) {
                    self.phase_a(child, ctes, outer, bindings);
                }
            }
        }
    }

    /// One `FROM`/`JOIN` relation item.
    fn relation_item(
        &mut self,
        node: Node<'a>,
        ctes: &[Binding],
        outer: &Frames,
        bindings: &mut Vec<Binding>,
    ) {
        let mut parts: Vec<(String, Range<usize>)> = Vec::new();
        let mut alias: Option<(String, Range<usize>)> = None;
        let mut subquery = None;
        for child in kids(node) {
            match child.kind() {
                "object_reference" => parts = self.identifier_parts(child),
                "identifier" => alias = Some((self.text(child.byte_range()), child.byte_range())),
                "subquery" => subquery = Some(child),
                _ => {}
            }
        }

        if let Some(sub) = subquery {
            // FROM-subquery: columns derive from its select list; its body
            // analysis is phase B's job.
            let columns = self.select_list_columns(sub);
            bindings.push(Binding {
                alias: alias.map(|(a, _)| a.into()),
                name: None,
                schema: None,
                object: None,
                columns,
                range: node.byte_range(),
            });
            return;
        }
        if parts.is_empty() {
            return;
        }

        let names: Vec<String> = parts.iter().map(|(n, _)| n.clone()).collect();
        let range = object_span(&parts);
        match self.resolve_relation(&names, ctes, outer) {
            Resolution::Cte(cte) => bindings.push(Binding {
                alias: alias.map(|(a, _)| a.into()),
                name: cte.name.clone(),
                schema: None,
                object: None,
                columns: cte.columns.clone(),
                range,
            }),
            Resolution::Catalog { idx } => bindings.push(Binding {
                alias: alias.map(|(a, _)| a.into()),
                name: Some(
                    self.catalog
                        .object(idx)
                        .map(|e| e.name().to_string())
                        .unwrap_or_default()
                        .into_boxed_str(),
                ),
                schema: Some(
                    self.catalog
                        .object(idx)
                        .and_then(|e| e.schema().map(|s| s.to_string()))
                        .unwrap_or_default()
                        .into_boxed_str(),
                ),
                object: Some(idx),
                columns: self.catalog_columns(idx),
                range,
            }),
            Resolution::None => {
                if self.loaded {
                    self.diags.push(Diagnostic::new(
                        DiagnosticKind::UnknownRelation,
                        range.clone(),
                        format!("unknown relation `{}`", names.join(".")),
                    ));
                }
                // Bind anyway with no column knowledge: references against
                // it stay silent, stopping cascading noise.
                bindings.push(Binding {
                    alias: alias.map(|(a, _)| a.into()),
                    name: Some(last_name(&names)),
                    schema: None,
                    object: None,
                    columns: Vec::new(),
                    range,
                });
            }
        }
    }

    /// A `LATERAL` join item: `(subquery)` or a function/invocation such as
    /// `generate_series(…)`. The alias is the direct trailing `identifier`
    /// child of the lateral node (the item's own identifiers are nested).
    /// Function items carry no column knowledge yet — stage 5's problem —
    /// but their alias must still bind so `g.x` does not report a missing
    /// relation.
    fn lateral_item(&mut self, node: Node<'a>, bindings: &mut Vec<Binding>) {
        let alias = direct(node, "identifier")
            .first()
            .map(|c| self.text(c.byte_range()));
        let columns = direct(node, "subquery")
            .first()
            .map(|sub| self.select_list_columns(*sub))
            .unwrap_or_default();
        if alias.is_none() && columns.is_empty() {
            return;
        }
        bindings.push(Binding {
            alias: alias.map(Into::into),
            name: None,
            schema: None,
            object: None,
            columns,
            range: node.byte_range(),
        });
    }

    /// `INSERT INTO t …` keeps its target as a direct `object_reference`
    /// child of the `insert` node.
    fn dml_target(
        &mut self,
        node: Node<'a>,
        ctes: &[Binding],
        outer: &Frames,
        bindings: &mut Vec<Binding>,
    ) {
        let Some(target) = kids(node)
            .into_iter()
            .find(|c| c.kind() == "object_reference")
        else {
            return;
        };
        self.bind_target(target, None, ctes, outer, bindings);
    }

    /// `DELETE FROM t …` keeps its target in the statement-level `from`
    /// sibling: the `delete` node itself holds only `keyword_delete`.
    fn dml_from(
        &mut self,
        from: Node<'a>,
        ctes: &[Binding],
        outer: &Frames,
        bindings: &mut Vec<Binding>,
    ) {
        let Some(target) = direct(from, "object_reference").first().copied() else {
            return;
        };
        let alias = direct(from, "identifier")
            .first()
            .map(|c| self.text(c.byte_range()));
        self.bind_target(target, alias, ctes, outer, bindings);
    }

    fn bind_target(
        &mut self,
        target: Node<'a>,
        alias: Option<String>,
        ctes: &[Binding],
        outer: &Frames,
        bindings: &mut Vec<Binding>,
    ) {
        let parts = self.identifier_parts(target);
        if parts.is_empty() {
            return;
        }
        let names: Vec<String> = parts.iter().map(|(n, _)| n.clone()).collect();
        let range = object_span(&parts);
        match self.resolve_relation(&names, ctes, outer) {
            Resolution::Cte(cte) => bindings.push(Binding {
                alias: alias.map(Into::into),
                name: cte.name.clone(),
                schema: None,
                object: None,
                columns: cte.columns.clone(),
                range,
            }),
            Resolution::Catalog { idx } => bindings.push(Binding {
                alias: alias.map(Into::into),
                name: Some(
                    self.catalog
                        .object(idx)
                        .map(|e| e.name().to_string())
                        .unwrap_or_default()
                        .into_boxed_str(),
                ),
                schema: Some(
                    self.catalog
                        .object(idx)
                        .and_then(|e| e.schema().map(|s| s.to_string()))
                        .unwrap_or_default()
                        .into_boxed_str(),
                ),
                object: Some(idx),
                columns: self.catalog_columns(idx),
                range,
            }),
            Resolution::None => {
                if self.loaded {
                    self.diags.push(Diagnostic::new(
                        DiagnosticKind::UnknownRelation,
                        range,
                        format!("unknown relation `{}`", names.join(".")),
                    ));
                }
            }
        }
    }

    // === Resolution ===

    /// Resolve one region's own column references: walk each top-level clause
    /// part, skip deferred territory (`subquery`, `cte`, a nested
    /// set-operation side), and lift `ERROR` fragments into syntax
    /// diagnostics (suppressing everything nested there).
    fn resolve_parts(&mut self, parts: &[Node<'a>], frame: &Frame, full: &Frames) {
        for part in parts {
            match part.kind() {
                "set_operation" => continue, // owns its sides' scopes
                "subquery" | "cte" => continue,
                "ERROR" => {
                    self.push_syntax_error(*part);
                    continue;
                }
                _ => {}
            }
            let mut stack = vec![*part];
            while let Some(node) = stack.pop() {
                match node.kind() {
                    "field" => self.resolve_field(node, frame, full),
                    "subquery" | "cte" => continue, // deferred territory
                    "ERROR" => {
                        self.push_syntax_error(node);
                        continue;
                    }
                    _ => {}
                }
                for child in kids(node) {
                    stack.push(child);
                }
            }
        }
    }

    fn push_syntax_error(&mut self, node: Node<'a>) {
        self.diags.push(Diagnostic::new(
            DiagnosticKind::SyntaxError,
            node.byte_range(),
            "syntax error".into(),
        ));
    }

    fn resolve_field(&mut self, node: Node<'a>, frame: &Frame, full: &Frames) {
        if !self.loaded {
            return; // silence rule: no catalog → no semantic diagnostics
        }
        // Identifiers in order: a bare reference has one; `a.col` two; a
        // schema-qualified `schema.rel.col` three (or more with a database
        // prefix — the last parts are what count).
        let refs = self.identifier_parts(node);
        if refs.is_empty() {
            return;
        }
        let names: Vec<String> = refs.iter().map(|(n, _)| n.clone()).collect();
        let last = refs.len() - 1;
        let column = names[last].clone();
        let column_range = refs[last].1.clone();

        if last == 0 {
            self.resolve_bare(&column, refs[0].1.clone(), frame, full);
            return;
        }
        if last == 1 {
            let qualifier = names[0].clone();
            if let Some(binding) = self.lookup_qualifier(&qualifier, frame, full) {
                self.resolve_against(binding, &column, column_range, &qualifier);
            } else {
                self.diags.push(Diagnostic::new(
                    DiagnosticKind::UnknownRelation,
                    refs[0].1.clone(),
                    format!("unknown relation or alias `{qualifier}`"),
                ));
            }
            return;
        }
        let schema = &names[last - 2];
        let rel = &names[last - 1];
        let rel_range = refs[last - 2].1.start..refs[last - 1].1.end;
        match self.resolve_catalog_rel(Some(schema), rel) {
            Some(idx) => self.resolve_catalog_columns(idx, &column, column_range),
            None => self.diags.push(Diagnostic::new(
                DiagnosticKind::UnknownRelation,
                rel_range,
                format!("unknown relation `{schema}.{rel}`"),
            )),
        }
    }

    /// Bare reference: resolve level by level, innermost first. The nearest
    /// scope that has *any* match decides — PostgreSQL only falls back to an
    /// enclosing query when the inner scope's sources do not provide the
    /// name, and ambiguity is only ever among sources of one level (so a
    /// correlated `id` inside a subquery whose own `FROM` has `id` does not
    /// clash with the outer table). CTEs are not candidates: their columns
    /// participate only when the CTE is genuinely `FROM`-bound.
    fn resolve_bare(&mut self, name: &str, range: Range<usize>, frame: &Frame, full: &Frames) {
        let mut any_columns = false;
        for source in std::iter::once(frame).chain(full[1..].iter()) {
            let mut hits: Vec<String> = Vec::new();
            for binding in &source.bindings {
                if binding.columns.is_empty() {
                    continue;
                }
                any_columns = true;
                let label = binding
                    .alias
                    .clone()
                    .unwrap_or_else(|| binding.name.clone().unwrap_or_default())
                    .into_string();
                if !binding.column_hits(name, self.catalog).is_empty() {
                    hits.push(label);
                }
            }
            match hits.len() {
                0 => continue, // not at this level: try the enclosing scope
                1 => return,   // the nearest scope decides
                _ => {
                    hits.sort();
                    hits.dedup();
                    self.diags.push(Diagnostic {
                        kind: DiagnosticKind::AmbiguousColumn,
                        severity: Severity::Warning,
                        range,
                        message: format!("ambiguous column `{name}` (matches {})", hits.join(", ")),
                    });
                    return;
                }
            }
        }
        if any_columns {
            self.diags.push(Diagnostic::new(
                DiagnosticKind::UnknownColumn,
                range,
                format!("unknown column `{name}`"),
            ));
        }
    }

    /// Qualified column against a resolved binding.
    fn resolve_against(
        &mut self,
        binding: &Binding,
        column: &str,
        range: Range<usize>,
        qualifier: &str,
    ) {
        if binding.columns.is_empty() {
            return; // no column knowledge (unresolved relation, DML target …)
        }
        let hits = binding.column_hits(column, self.catalog);
        match hits.len() {
            0 => self.diags.push(Diagnostic::new(
                DiagnosticKind::UnknownColumn,
                range,
                format!("column `{column}` not found in `{qualifier}`"),
            )),
            1 => {}
            _ => self.diags.push(Diagnostic {
                kind: DiagnosticKind::AmbiguousColumn,
                severity: Severity::Warning,
                range,
                message: format!("ambiguous column `{column}` within `{qualifier}`"),
            }),
        }
    }

    /// Schema-qualified column directly against a catalog relation
    /// (`schema.rel.col`).
    fn resolve_catalog_columns(&mut self, rel: u32, column: &str, range: Range<usize>) {
        let folded = column.to_lowercase();
        let hits: Vec<u32> = self
            .catalog
            .columns_of(rel)
            .iter()
            .filter(|c| c.name().to_lowercase() == folded)
            .filter_map(|c| self.catalog.by_id(c.id()))
            .collect();
        match hits.len() {
            0 => self.diags.push(Diagnostic::new(
                DiagnosticKind::UnknownColumn,
                range,
                format!("column `{column}` not found"),
            )),
            1 => {}
            _ => self.diags.push(Diagnostic {
                kind: DiagnosticKind::AmbiguousColumn,
                severity: Severity::Warning,
                range,
                message: format!("ambiguous column `{column}`"),
            }),
        }
    }

    /// Qualifier lookup: the region's own CTEs and bindings first, then the
    /// enclosing frames nearest first.
    fn lookup_qualifier<'f>(
        &self,
        qualifier: &str,
        frame: &'f Frame,
        full: &'f Frames,
    ) -> Option<&'f Binding> {
        if let Some(binding) = frame.qualifier(qualifier) {
            return Some(binding);
        }
        full[1..]
            .iter()
            .find_map(|outer| outer.qualifier(qualifier))
    }

    // === Relations ===

    /// The catalog's column refs of a resolved relation.
    fn catalog_columns(&self, rel_idx: u32) -> Vec<ColumnRef> {
        self.catalog
            .columns_of(rel_idx)
            .iter()
            .filter_map(|c| {
                self.catalog
                    .by_id(c.id())
                    .map(|index| ColumnRef::Catalog { index })
            })
            .collect()
    }

    /// Resolve a (possibly schema-qualified) relation name. CTE names shadow
    /// catalog relations at the unqualified level — the region's own ctes
    /// first, then the enclosing frames nearest first; for the catalog,
    /// exact match wins and a unique folded match is accepted.
    fn resolve_relation(&self, names: &[String], ctes: &[Binding], outer: &Frames) -> Resolution {
        if names.is_empty() {
            return Resolution::None;
        }
        if names.len() == 1 {
            let own = std::iter::once(ctes).chain(outer.iter().map(|f| f.ctes.as_slice()));
            for chain in own {
                if let Some(found) = chain.iter().find(|c| c.matches_qualifier(&names[0])) {
                    return Resolution::Cte(Box::new(found.clone()));
                }
            }
        }
        let schema = if names.len() >= 2 {
            Some(names[names.len() - 2].as_str())
        } else {
            None
        };
        let name = names.last().cloned().unwrap_or_default();
        match self.resolve_catalog_rel(schema, &name) {
            Some(idx) => Resolution::Catalog { idx },
            None => Resolution::None,
        }
    }

    /// Catalog-only relation resolution: exact first across schemas, then a
    /// unique folded match. `(idx, exact)` parking note: several folded
    /// matches (e.g. `users` beside quoted `"Users"`) resolve to nothing —
    /// rare, silenced here, TODO for the ambiguity rule.
    fn resolve_catalog_rel(&self, schema: Option<&str>, name: &str) -> Option<u32> {
        match schema {
            Some(schema) => {
                if let Some(idx) = self.catalog.relation_exact(schema, name) {
                    return Some(idx);
                }
                let hits = self.catalog.relation_folded(schema, name);
                if hits.len() == 1 {
                    return Some(hits[0]);
                }
                None
            }
            None => {
                for schema in self.catalog.schemas() {
                    if let Some(idx) = self.catalog.relation_exact(schema, name) {
                        return Some(idx);
                    }
                }
                let mut folded: Option<u32> = None;
                for schema in self.catalog.schemas() {
                    let hits = self.catalog.relation_folded(schema, name);
                    match hits.as_slice() {
                        [idx] => {
                            if folded.replace(*idx).is_some() {
                                return None; // a second schema folds in: ambiguous
                            }
                        }
                        [] => {}
                        _ => return None,
                    }
                }
                folded
            }
        }
    }

    // === CTEs and the wrapping select list ===

    /// A CTE binding: name, explicit column list when typed, else derived
    /// from the body's top select list (without analyzing the body).
    fn cte_binding(&mut self, cte: Node<'a>, ctes: &[Binding]) -> Binding {
        let _ = ctes; // earlier CTEs matter only when the body is analyzed
        let mut name: Option<(String, Range<usize>)> = None;
        let mut explicit: Vec<ColumnRef> = Vec::new();
        let mut depth: usize = 0;
        let mut after_as = false;
        let mut body = None;
        for child in kids(cte) {
            match child.kind() {
                "(" => depth += 1,
                ")" => depth = depth.saturating_sub(1),
                "keyword_as" => after_as = true,
                "identifier" if name.is_none() && depth == 0 && !after_as => {
                    name = Some((self.text(child.byte_range()), child.byte_range()))
                }
                "identifier" if name.is_some() && depth == 1 && !after_as => {
                    explicit.push(ColumnRef::Local {
                        name: self.text(child.byte_range()).into_boxed_str(),
                        data_type: None,
                    })
                }
                "statement" if body.is_none() => body = Some(child),
                _ => {}
            }
        }
        let columns = if !explicit.is_empty() {
            explicit
        } else if let Some(body) = body {
            self.select_list_columns(body)
        } else {
            Vec::new()
        };
        Binding {
            alias: None,
            name: name.as_ref().map(|(n, _)| n.clone().into_boxed_str()),
            schema: None,
            object: None,
            columns,
            range: name
                .as_ref()
                .map(|(_, r)| r.clone())
                .unwrap_or_else(|| cte.byte_range()),
        }
    }

    /// The output columns of a region's top select list: one `Local` column
    /// per term whose expression is a plain `field` — the term's last
    /// identifier (the `AS` alias when stated, else the column's name).
    fn select_list_columns(&self, region: Node<'a>) -> Vec<ColumnRef> {
        let mut columns = Vec::new();
        let Some(select) = find_select(region) else {
            return columns;
        };
        let Some(expr) = direct(select, "select_expression").first().copied() else {
            return columns;
        };
        for term in direct(expr, "term") {
            if direct(term, "field").is_empty() {
                continue; // literals, invocations, nested queries: unknown
            }
            if let Some(range) = self.identifier_parts(term).last().map(|(_, r)| r) {
                columns.push(ColumnRef::Local {
                    name: self.text(range.clone()).into_boxed_str(),
                    data_type: None,
                });
            }
        }
        columns
    }
}

// === Free functions ===

/// Top-level statement regions: root children beyond bare separators.
fn regions(root: Node<'_>) -> Vec<Node<'_>> {
    kids(root)
        .into_iter()
        .filter(|c| !matches!(c.kind(), ";" | ","))
        .filter(|c| c.child_count() > 0)
        .collect()
}

/// Children as an owned vec (borrow-simple file-wide).
fn kids(node: Node<'_>) -> Vec<Node<'_>> {
    let mut cursor = node.walk();
    node.children(&mut cursor).collect()
}

/// Direct children with `kind`.
fn direct<'n>(node: Node<'n>, kind: &str) -> Vec<Node<'n>> {
    kids(node)
        .into_iter()
        .filter(|c| c.kind() == kind)
        .collect()
}

/// The top `select` of a region: its direct child, descending through
/// statement/set-operation/subquery wrappers; CTE bodies and `ERROR`
/// fragments are not candidates.
fn find_select<'n>(region: Node<'n>) -> Option<Node<'n>> {
    if region.kind() == "select" {
        return Some(region);
    }
    for child in kids(region) {
        match child.kind() {
            "select" => return Some(child),
            "statement" | "set_operation" | "subquery" => {
                if let Some(inner) = find_select(child) {
                    return Some(inner);
                }
            }
            _ => {}
        }
    }
    None
}

/// Split a `set_operation` into its sides: each `select` (or nested
/// `set_operation`) starts a side, and the clause nodes that follow it
/// (`from`, `where`, `order_by`, …) belong to that side. Separators
/// (`keyword_union` family) and the parentheses of a parenthesized side are
/// dropped.
fn set_operation_sides(op: Node<'_>) -> Vec<Vec<Node<'_>>> {
    let mut sides: Vec<Vec<Node<'_>>> = Vec::new();
    for child in kids(op) {
        match child.kind() {
            "select" | "set_operation" => sides.push(vec![child]),
            // UNION / INTERSECT / EXCEPT, ALL/DISTINCT, and parens.
            k if k.starts_with("keyword_") || k == "(" || k == ")" => {}
            _ => {
                if let Some(side) = sides.last_mut() {
                    side.push(child);
                }
            }
        }
    }
    sides
}

/// Byte span covered by a region's top-level parts.
fn parts_range(parts: &[Node<'_>]) -> Range<usize> {
    match (parts.first(), parts.last()) {
        (Some(first), Some(last)) => first.byte_range().start..last.byte_range().end,
        _ => 0..0,
    }
}

/// Is this `from` node the target clause of a `DELETE`? The grammar keeps
/// `delete` (keyword only) and its `from` as *siblings* under `statement`.
fn is_delete_target_clause(node: Node<'_>) -> bool {
    node.parent()
        .is_some_and(|parent| kids(parent).iter().any(|c| c.kind() == "delete"))
}

/// The parenthesized `statement` body of a `cte` node.
fn cte_body<'n>(cte: Node<'n>) -> Option<Node<'n>> {
    direct(cte, "statement").first().copied()
}

/// Byte span of a multi-part object reference: first part's start through
/// the last part's end.
fn object_span(parts: &[(String, Range<usize>)]) -> Range<usize> {
    if parts.is_empty() {
        0..0
    } else {
        parts[0].1.start..parts[parts.len() - 1].1.end
    }
}

/// All `identifier` descendants of a node, in source order. Nested
/// `subquery`/`cte` subtrees are excluded — their identifiers belong to
/// other scopes.
fn collect_identifier_ranges(node: Node<'_>, out: &mut Vec<Range<usize>>) {
    for child in kids(node) {
        match child.kind() {
            "identifier" => out.push(child.byte_range()),
            "subquery" | "cte" => {}
            _ => collect_identifier_ranges(child, out),
        }
    }
}

/// The last part's text of a qualified name (`db.schema.rel` → `rel`).
fn last_name(names: &[String]) -> Box<str> {
    names.last().cloned().unwrap_or_default().into_boxed_str()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempr_domain::ids::{ConnectionId, SchemaObjectId, SchemaSnapshotId};
    use tempr_domain::schema::{SchemaObject, SchemaSnapshot};

    /// Tables of `sakila`-flavored shape: `(name, columns)` in `public`.
    /// `None` columns build an empty-catalog fixture for the silence rule.
    fn catalog_with(tables: &[(&str, &[&str])]) -> Arc<CatalogCache> {
        let mut objects: Vec<SchemaObject> = Vec::new();
        for (name, columns) in tables {
            let table_id = SchemaObjectId::new();
            objects.push(SchemaObject::Table {
                id: table_id,
                schema: "public".into(),
                name: (*name).into(),
                estimated_rows: None,
            });
            for (ordinal, column) in columns.iter().enumerate() {
                objects.push(SchemaObject::Column {
                    id: SchemaObjectId::new(),
                    parent_id: table_id,
                    name: (*column).into(),
                    data_type: "int8".into(),
                    nullable: false,
                    ordinal,
                    default: None,
                });
            }
        }
        Arc::new(CatalogCache::from_snapshot(&SchemaSnapshot {
            id: SchemaSnapshotId::new(),
            connection_id: ConnectionId::new(),
            version: 1,
            fetched_at: chrono::Utc::now(),
            objects,
            keywords: vec![],
            fingerprints: vec![],
        }))
    }

    fn empty_catalog() -> Arc<CatalogCache> {
        catalog_with(&[])
    }

    fn analyze(
        _catalog: &Arc<CatalogCache>,
        catalog_of: &Arc<CatalogCache>,
        sql: &str,
    ) -> Analysis {
        let engine = SemanticEngine::new(catalog_of.clone());
        let mut buffer = Buffer::new(SqlFileId::new(), sql);
        (*engine.analyze(SqlFileId::new(), &mut buffer)).clone()
    }

    fn loaded() -> Arc<CatalogCache> {
        catalog_with(&[
            ("users", &["id", "name"]),
            ("orders", &["id", "user_id", "total"]),
            ("admins", &["id"]),
            ("vals", &["x"]),
        ])
    }

    /// Message kinds in byte order — the spec §8 table reads across kinds.
    fn kinds(analysis: &Analysis) -> Vec<(DiagnosticKind, String)> {
        analysis
            .diagnostics
            .iter()
            .map(|d| (d.kind, d.message.clone()))
            .collect()
    }

    #[test]
    fn clean_query_over_aliases_and_joins() {
        let analysis = analyze(
            &loaded(),
            &loaded(),
            "select u.id, o.total from users u join orders o on o.user_id = u.id where o.status is null;",
        );
        // `status` is not in the fixture's `orders`.
        assert_eq!(
            kinds(&analysis),
            vec![(
                DiagnosticKind::UnknownColumn,
                "column `status` not found in `o`".into()
            )]
        );

        let clean = analyze(
            &loaded(),
            &loaded(),
            "select u.id, o.total from users u join orders o on o.user_id = u.id;",
        );
        assert!(clean.diagnostics.is_empty(), "{:?}", kinds(&clean));
    }

    #[test]
    fn bare_column_unique_match_resolves() {
        let analysis = analyze(&loaded(), &loaded(), "select name from users;");
        assert!(analysis.diagnostics.is_empty(), "{:?}", kinds(&analysis));
    }

    #[test]
    fn bare_column_across_two_tables_is_a_warning() {
        let analysis = analyze(
            &loaded(),
            &loaded(),
            "select id from users u join orders o on o.user_id = u.id;",
        );
        assert_eq!(
            kinds(&analysis),
            vec![(
                DiagnosticKind::AmbiguousColumn,
                "ambiguous column `id` (matches o, u)".into()
            )]
        );
        assert_eq!(analysis.diagnostics[0].severity, Severity::Warning);
    }

    #[test]
    fn unknown_bare_column_is_an_error() {
        let analysis = analyze(&loaded(), &loaded(), "select zz from users;");
        assert_eq!(
            kinds(&analysis),
            vec![(DiagnosticKind::UnknownColumn, "unknown column `zz`".into())]
        );
        assert_eq!(analysis.diagnostics[0].severity, Severity::Error);
    }

    #[test]
    fn unknown_qualifier_reports_the_but_not_its_column() {
        let analysis = analyze(&loaded(), &loaded(), "select x.name from users u;");
        assert_eq!(
            kinds(&analysis),
            vec![(
                DiagnosticKind::UnknownRelation,
                "unknown relation or alias `x`".into()
            )]
        );
    }

    #[test]
    fn unknown_qualified_column_is_an_error() {
        let analysis = analyze(&loaded(), &loaded(), "select u.zz from users u;");
        assert_eq!(
            kinds(&analysis),
            vec![(
                DiagnosticKind::UnknownColumn,
                "column `zz` not found in `u`".into()
            )]
        );
    }

    #[test]
    fn cte_chain_binds_columns_forward() {
        let analysis = analyze(
            &loaded(),
            &loaded(),
            "with a as (select id from users), b as (select id from a) select b.id from b;",
        );
        assert!(analysis.diagnostics.is_empty(), "{:?}", kinds(&analysis));
    }

    #[test]
    fn cte_explicit_column_list_wins_the_header() {
        let analysis = analyze(
            &loaded(),
            &loaded(),
            "with c(v) as (select id from users) select c.v from c;",
        );
        assert!(analysis.diagnostics.is_empty(), "{:?}", kinds(&analysis));
        // And the CTE's columns do NOT leak the underlying driver name:
        let analysis = analyze(
            &loaded(),
            &loaded(),
            "with c(v) as (select id from users) select c.id from c;",
        );
        assert_eq!(
            kinds(&analysis),
            vec![(
                DiagnosticKind::UnknownColumn,
                "column `id` not found in `c`".into()
            )]
        );
    }

    #[test]
    fn correlated_subquery_resolves_outer_aliases() {
        let analysis = analyze(
            &loaded(),
            &loaded(),
            "select u.name from users u where exists (select 1 from orders o where o.user_id = u.id);",
        );
        assert!(analysis.diagnostics.is_empty(), "{:?}", kinds(&analysis));
    }

    #[test]
    fn from_subquery_alias_resolves_derived_columns() {
        let analysis = analyze(
            &loaded(),
            &loaded(),
            "select t.x from (select x from vals) t where t.x > 0;",
        );
        assert!(analysis.diagnostics.is_empty(), "{:?}", kinds(&analysis));
    }

    #[test]
    fn aliased_select_output_columns() {
        let analysis = analyze(
            &loaded(),
            &loaded(),
            "select u.id as uid, u.name from users u where uid > 0;",
        );
        // `uid` resolves because the outer select list names it; bare `uid`
        // without FROM-side columns is unknown instead — the alias only
        // marks the output column, no bare-column binding.
        assert_eq!(
            kinds(&analysis),
            vec![(DiagnosticKind::UnknownColumn, "unknown column `uid`".into())]
        );
    }

    #[test]
    fn set_operations_share_one_statement_scope() {
        let analysis = analyze(
            &loaded(),
            &loaded(),
            "select id from users union select id from admins order by id limit 10;",
        );
        assert!(analysis.diagnostics.is_empty(), "{:?}", kinds(&analysis));
    }

    #[test]
    fn stars_never_diagnose() {
        let analysis = analyze(&loaded(), &loaded(), "select *, u.name from users u;");
        assert!(analysis.diagnostics.is_empty(), "{:?}", kinds(&analysis));
        let analysis = analyze(&loaded(), &loaded(), "select u.* from users u;");
        assert!(analysis.diagnostics.is_empty(), "{:?}", kinds(&analysis));
    }

    #[test]
    fn no_catalog_silences_semantics_but_not_syntax() {
        // Unknown alias + unknown column + an unparseable `limit` parameter.
        let analysis = analyze(
            &empty_catalog(),
            &empty_catalog(),
            "select x.name from users u where zz is null;",
        );
        assert!(
            analysis.diagnostics.is_empty(),
            "silence rule violated: {:?}",
            kinds(&analysis)
        );

        let analysis = analyze(
            &empty_catalog(),
            &empty_catalog(),
            "select 1 from users limit $semantically_noiseless;",
        );
        let ks = kinds(&analysis);
        assert!(
            ks.iter().all(|(k, _)| *k == DiagnosticKind::SyntaxError),
            "only syntax diagnostics may appear: {:?}",
            ks
        );
    }

    #[test]
    fn syntax_error_fragments_report_once_and_suppress_semantics() {
        let analysis = analyze(&loaded(), &loaded(), "select 1 from users limit $3;");
        let ks = kinds(&analysis);
        assert_eq!(
            ks,
            vec![(DiagnosticKind::SyntaxError, "syntax error".into())]
        );
    }

    #[test]
    fn delete_and_insert_targets_resolve() {
        let analysis = analyze(
            &loaded(),
            &loaded(),
            "insert into users (id) values (1); delete from users;",
        );
        assert!(analysis.diagnostics.is_empty(), "{:?}", kinds(&analysis));
        assert_eq!(analysis.scopes.len(), 2);
        for scope in &analysis.scopes {
            // Insert's target is a direct child of `insert`; delete's lives
            // in the statement-level `from` sibling.
            assert_eq!(
                scope.bindings.first().and_then(|b| b.name.as_deref()),
                Some("users"),
                "both DML targets bind"
            );
        }
    }

    #[test]
    fn subquery_bodies_bind_their_own_from_clause() {
        // Qualified reference through the subquery's own alias.
        let analysis = analyze(
            &loaded(),
            &loaded(),
            "select 1 from users u where exists (select o.user_id from orders o where o.id = u.id);",
        );
        assert!(analysis.diagnostics.is_empty(), "{:?}", kinds(&analysis));

        // A bad column is reported against the subquery's alias, not as an
        // unknown relation from the outer scope.
        let analysis = analyze(
            &loaded(),
            &loaded(),
            "select 1 from users u where exists (select o.zz from orders o where o.id = u.id);",
        );
        assert_eq!(
            kinds(&analysis),
            vec![(
                DiagnosticKind::UnknownColumn,
                "column `zz` not found in `o`".into()
            )]
        );
    }

    #[test]
    fn subquery_bare_columns_prefer_the_inner_scope() {
        // `id` exists at both levels: the subquery's own `orders` wins, so no
        // cross-level ambiguity warning (PostgreSQL resolves innermost first
        // and only falls back to the outer query for correlated references).
        let analysis = analyze(
            &loaded(),
            &loaded(),
            "select id from users u where id in (select id from orders o);",
        );
        assert!(analysis.diagnostics.is_empty(), "{:?}", kinds(&analysis));

        // Present at neither level: exactly one unknown-column error.
        let analysis = analyze(
            &loaded(),
            &loaded(),
            "select (select zzz from orders o) from users u;",
        );
        assert_eq!(
            kinds(&analysis),
            vec![(DiagnosticKind::UnknownColumn, "unknown column `zzz`".into())]
        );
    }

    #[test]
    fn set_operation_sides_bind_and_resolve_their_own_sources() {
        let analysis = analyze(
            &loaded(),
            &loaded(),
            "select zz from users union select id from admins;",
        );
        assert_eq!(
            kinds(&analysis),
            vec![(DiagnosticKind::UnknownColumn, "unknown column `zz`".into())]
        );
        assert_eq!(analysis.scopes[0].children.len(), 2);
        for side in &analysis.scopes[0].children {
            assert!(
                !side.bindings.is_empty(),
                "each set-operation side binds its own FROM"
            );
        }
    }

    #[test]
    fn from_subquery_alias_columns_are_checked() {
        let analysis = analyze(
            &loaded(),
            &loaded(),
            "select t.zz from (select id from users) t;",
        );
        assert_eq!(
            kinds(&analysis),
            vec![(
                DiagnosticKind::UnknownColumn,
                "column `zz` not found in `t`".into()
            )]
        );
        let derived = analysis.scopes[0]
            .bindings
            .iter()
            .find(|b| b.alias.as_deref() == Some("t"))
            .expect("derived-table binding");
        assert_eq!(derived.columns.len(), 1, "derived columns feed stage 5");
    }

    #[test]
    fn lateral_subqueries_and_function_items_bind_their_alias() {
        let analysis = analyze(
            &loaded(),
            &loaded(),
            "select g.zz from users u join lateral (select id from orders) g on true;",
        );
        assert_eq!(
            kinds(&analysis),
            vec![(
                DiagnosticKind::UnknownColumn,
                "column `zz` not found in `g`".into()
            )]
        );

        // `cross join lateral` is a distinct grammar node and a function item
        // has no column knowledge yet — but its alias must resolve instead of
        // being reported as a missing relation.
        let analysis = analyze(
            &loaded(),
            &loaded(),
            "select g.x from users u cross join lateral generate_series(1, 3) g;",
        );
        assert!(analysis.diagnostics.is_empty(), "{:?}", kinds(&analysis));
    }

    #[test]
    fn alias_hides_the_relation_name_and_schemas_are_not_qualifiers() {
        let analysis = analyze(&loaded(), &loaded(), "select users.id from users u;");
        assert_eq!(
            kinds(&analysis),
            vec![(
                DiagnosticKind::UnknownRelation,
                "unknown relation or alias `users`".into()
            )]
        );

        let analysis = analyze(&loaded(), &loaded(), "select public.name from users;");
        assert_eq!(
            kinds(&analysis),
            vec![(
                DiagnosticKind::UnknownRelation,
                "unknown relation or alias `public`".into()
            )]
        );
    }

    #[test]
    fn reopened_buffer_never_hits_another_buffers_cached_analysis() {
        let engine = SemanticEngine::new(loaded());
        let file = SqlFileId::new();
        let mut first = Buffer::new(file, "select zz from users;");
        let stale = engine.analyze(file, &mut first);
        assert_eq!(stale.diagnostics.len(), 1);

        // A new Buffer for the same file has its own revision, so it must not
        // reuse the previous instance's entry.
        let mut reopened = Buffer::new(file, "select name from users;");
        let fresh = engine.analyze(file, &mut reopened);
        assert!(!Arc::ptr_eq(&stale, &fresh));
        assert!(fresh.diagnostics.is_empty(), "{:?}", kinds(&fresh));
    }

    #[test]
    fn schema_qualified_columns_resolve_through_the_catalog() {
        let analysis = analyze(
            &loaded(),
            &loaded(),
            "select u.name from users u where public.users.id = u.id;",
        );
        assert!(analysis.diagnostics.is_empty(), "{:?}", kinds(&analysis));
        let analysis = analyze(&loaded(), &loaded(), "select public.users.zz from users;");
        assert_eq!(
            kinds(&analysis),
            vec![(
                DiagnosticKind::UnknownColumn,
                "column `zz` not found".into()
            )]
        );
    }

    #[test]
    fn analysis_cache_reuses_revision_and_rejects_edits() {
        let engine = SemanticEngine::new(loaded());
        let file = SqlFileId::new();
        let mut buffer = Buffer::new(file, "select zz from users;");
        let first = engine.analyze(file, &mut buffer);
        let second = engine.analyze(file, &mut buffer);
        assert!(Arc::ptr_eq(&first, &second), "same revision → same Arc");

        buffer.edit(&[(0..0, " ")]).unwrap();
        let third = engine.analyze(file, &mut buffer);
        assert!(!Arc::ptr_eq(&first, &third), "edit → fresh analysis");
        assert_eq!(third.diagnostics.len(), first.diagnostics.len());
    }

    #[test]
    fn region_analysis_matches_the_full_scope_chain() {
        let engine = SemanticEngine::new(loaded());
        let file = SqlFileId::new();
        let sql = "select id from users;\nselect name from users u where u.id = 1";
        let mut buffer = Buffer::new(file, sql);
        let offset = sql.find("name").expect("needle") + "name".len();

        let region = engine
            .analyzed_at(file, buffer.revision(), offset, &mut buffer)
            .0;
        let full = engine.analyze(file, &mut buffer);

        assert_eq!(
            region.scope_chain(offset),
            full.scope_chain(offset),
            "the hot path must see the same scopes as diagnostics"
        );
        assert_eq!(region.scopes.len(), 1, "unrelated statements are skipped");
        assert_eq!(full.scopes.len(), 2);
    }

    #[test]
    fn stats_sanity_on_fixture() {
        assert_eq!(loaded().stats().relations, 4);
    }
}
