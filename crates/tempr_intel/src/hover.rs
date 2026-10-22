//! Hover resolution (spec §5.3). Hover is synchronous and never queries the
//! database.
//!
//! Contents are plain text with `\n` separators; the UI renders them as lines.

use std::ops::Range;

use crate::analysis::{Analysis, Binding, ColumnRef};
use crate::catalog::{CatalogCache, CatalogEntry};
use crate::completion::{is_ident_char, qualifier_before};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HoverInfo {
    /// Plain-text card contents (`\n`-separated lines).
    pub contents: String,
    /// Byte range of the symbol under the cursor (quotes included for a
    /// quoted identifier).
    pub range: Range<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Reference {
    pub range: Range<usize>,
    pub path: Vec<String>,
    pub word: String,
}

/// The reference under `offset`, if any. The cursor may sit inside the word
/// or immediately after it.
pub(crate) fn reference_at(text: &str, offset: usize) -> Option<Reference> {
    let offset = offset.min(text.len());
    if !text.is_char_boundary(offset) {
        return None;
    }
    let (word_range, word) = word_at(text, offset)?;
    let (path, path_start) = qualifier_before(text, word_range.start).unzip();
    let start = path_start.unwrap_or(word_range.start);
    Some(Reference {
        range: start..word_range.end,
        path: path.unwrap_or_default(),
        word,
    })
}

fn word_at(text: &str, offset: usize) -> Option<(Range<usize>, String)> {
    if text.is_empty() {
        return None;
    }
    let at = text[offset..].chars().next();
    let prev = text[..offset].char_indices().next_back();

    let (start, quoted) = match at {
        Some('"') => (offset, true),
        Some(c) if is_ident_char(c) => (offset, false),
        _ => match prev {
            Some((i, '"')) => (opening_quote(text, i)?, true),
            Some((i, c)) if is_ident_char(c) => (i, false),
            _ => return None,
        },
    };

    if quoted {
        let end = closing_quote(text, start)?;
        let inner = text[start + 1..end].replace("\"\"", "\"");
        return (!inner.is_empty()).then_some((start..end + 1, inner));
    }

    let mut start = start;
    while let Some((i, c)) = text[..start].char_indices().next_back() {
        if is_ident_char(c) {
            start = i;
        } else {
            break;
        }
    }
    let mut end = offset.max(start);
    let base = end;
    for (i, c) in text[base..].char_indices() {
        if is_ident_char(c) {
            end = base + i + c.len_utf8();
        } else {
            break;
        }
    }
    if start == end {
        return None;
    }
    Some((start..end, text[start..end].to_string()))
}

fn opening_quote(text: &str, close: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut i = close;
    while i > 0 {
        i -= 1;
        if bytes[i] == b'"' {
            let escaped = i > 0 && bytes[i - 1] == b'"';
            if escaped {
                i -= 1;
                continue;
            }
            return Some(i);
        }
    }
    None
}

fn closing_quote(text: &str, open: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut i = open + 1;
    while i < bytes.len() {
        if bytes[i] == b'"' {
            if i + 1 < bytes.len() && bytes[i + 1] == b'"' {
                i += 2;
                continue;
            }
            return Some(i);
        }
        i += 1;
    }
    None
}

/// Resolve a reference against the analysis and catalog; `None` when there is
/// nothing to say (whitespace, an unknown symbol, no catalog).
pub(crate) fn resolve(
    catalog: &CatalogCache,
    analysis: &Analysis,
    offset: usize,
    reference: &Reference,
) -> Option<String> {
    let word = reference.word.as_str();
    match reference.path.as_slice() {
        [] => resolve_bare(catalog, analysis, offset, word),
        [qualifier] => resolve_qualified(catalog, analysis, offset, qualifier, word),
        // PostgreSQL's `db.schema.rel.col` form puts the database first; only
        // the last two segments qualify the relation.
        [.., schema, relation] => resolve_schema_relation(catalog, schema, relation, word),
    }
}

fn resolve_bare(
    catalog: &CatalogCache,
    analysis: &Analysis,
    offset: usize,
    word: &str,
) -> Option<String> {
    let chain = analysis.scope_chain(offset);
    // PostgreSQL resolves a bare column one nesting level at a time: the
    // innermost scope with any match decides, so an inner `id` is never
    // ambiguous with an outer `id`.
    for scope in chain.iter().rev() {
        let hits: Vec<(&Binding, ColumnRef)> = scope
            .bindings
            .iter()
            .flat_map(|binding| {
                binding
                    .column_hits(word, catalog)
                    .into_iter()
                    .map(move |column| (binding, column))
            })
            .collect();
        match hits.as_slice() {
            [] => continue,
            [(binding, column)] => return column_card(catalog, column, Some(binding)),
            many => {
                let names: Vec<String> = many
                    .iter()
                    .filter_map(|(binding, _)| {
                        binding_owner(catalog, binding).map(|owner| format!("{owner}.{word}"))
                    })
                    .collect();
                let mut message = format!("column `{word}` is ambiguous");
                if !names.is_empty() {
                    message.push('\n');
                    message.push_str(&names.join("\n"));
                }
                return Some(message);
            }
        }
    }

    if let Some(binding) = chain
        .iter()
        .rev()
        .flat_map(|scope| scope.bindings.iter())
        .find(|b| b.matches_qualifier(word))
    {
        if let Some(index) = binding.object
            && binding.alias.is_none()
            && let Some(card) = relation_card(catalog, index)
        {
            return Some(card);
        }
        return Some(binding_card(catalog, binding));
    }

    let relations = relations_folded(catalog, word);
    match relations.as_slice() {
        [idx] => return relation_card(catalog, *idx),
        [] => {}
        many => {
            let names: Vec<String> = many
                .iter()
                .filter_map(|idx| catalog.object(*idx))
                .map(|entry| format!("{}.{}", entry.schema().unwrap_or("?"), entry.name()))
                .collect();
            return Some(format!(
                "relation `{word}` is ambiguous\n{}",
                names.join("\n")
            ));
        }
    }

    if let Some(entry) = function_named(catalog, word) {
        return Some(function_card(entry));
    }
    if catalog
        .schemas()
        .iter()
        .any(|s| s.eq_ignore_ascii_case(word))
    {
        return Some(schema_card(catalog, word));
    }
    None
}

fn resolve_qualified(
    catalog: &CatalogCache,
    analysis: &Analysis,
    offset: usize,
    qualifier: &str,
    word: &str,
) -> Option<String> {
    if let Some(binding) = qualifier_binding(analysis, offset, qualifier) {
        let hits = binding.column_hits(word, catalog);
        return match hits.as_slice() {
            [column] => column_card(catalog, column, Some(binding)),
            [] => None,
            many => {
                let names = many
                    .iter()
                    .filter_map(|c| column_name(catalog, c))
                    .collect::<Vec<_>>()
                    .join(", ");
                Some(format!("column `{qualifier}.{word}` is ambiguous\n{names}"))
            }
        };
    }

    let relations = catalog.relation_folded(qualifier, word);
    match relations.as_slice() {
        [idx] => return relation_card(catalog, *idx),
        [] => {}
        many => {
            let names = many
                .iter()
                .filter_map(|idx| catalog.object(*idx))
                .map(|entry| format!("{}.{}", entry.schema().unwrap_or("?"), entry.name()))
                .collect::<Vec<_>>()
                .join("\n");
            return Some(format!(
                "relation `{qualifier}.{word}` is ambiguous\n{names}"
            ));
        }
    }

    let relations = relations_folded(catalog, qualifier);
    match relations.as_slice() {
        [idx] => {
            let rel = *idx;
            let columns = catalog.column_folded(rel, word);
            if let Some(column) = columns.first().and_then(|i| catalog.object(*i)) {
                return Some(catalog_column_card(catalog, rel, column));
            }
        }
        [] => {}
        many => {
            for idx in many {
                let columns = catalog.column_folded(*idx, word);
                if let Some(column) = columns.first().and_then(|i| catalog.object(*i)) {
                    return Some(catalog_column_card(catalog, *idx, column));
                }
            }
        }
    }
    None
}

fn resolve_schema_relation(
    catalog: &CatalogCache,
    schema: &str,
    relation: &str,
    word: &str,
) -> Option<String> {
    let relations = catalog.relation_folded(schema, relation);
    let rel = *relations.first()?;
    let columns = catalog.column_folded(rel, word);
    match columns.first() {
        Some(index) => catalog
            .object(*index)
            .map(|column| catalog_column_card(catalog, rel, column)),
        None => relation_card(catalog, rel),
    }
}

fn qualifier_binding<'a>(
    analysis: &'a Analysis,
    offset: usize,
    qualifier: &str,
) -> Option<&'a Binding> {
    analysis
        .scope_chain(offset)
        .iter()
        .rev()
        .find_map(|scope| scope.qualifier_binding(qualifier))
}

fn relations_folded(catalog: &CatalogCache, name: &str) -> Vec<u32> {
    let mut found: Vec<u32> = catalog
        .schemas()
        .iter()
        .flat_map(|schema| catalog.relation_folded(schema, name))
        .collect();
    found.sort_unstable();
    found.dedup();
    found
}

fn function_named<'a>(catalog: &'a CatalogCache, name: &str) -> Option<&'a CatalogEntry> {
    let (indices, _) = catalog.prefix_search(name, 500);
    indices
        .into_iter()
        .filter_map(|idx| catalog.object(idx))
        .find(|entry| {
            matches!(entry, CatalogEntry::Function { .. })
                && entry.name().eq_ignore_ascii_case(name)
        })
}

fn binding_owner(catalog: &CatalogCache, binding: &Binding) -> Option<String> {
    if let Some(index) = binding.object
        && let Some(entry) = catalog.object(index)
    {
        return Some(format!(
            "{}.{}",
            entry.schema().unwrap_or("public"),
            entry.name()
        ));
    }
    binding
        .alias
        .as_deref()
        .or(binding.name.as_deref())
        .map(str::to_string)
}

fn column_name(catalog: &CatalogCache, column: &ColumnRef) -> Option<String> {
    match column {
        ColumnRef::Catalog { index } => catalog.object(*index).map(|e| e.name().to_string()),
        ColumnRef::Local { name, .. } => Some(name.to_string()),
    }
}

fn column_card(
    catalog: &CatalogCache,
    column: &ColumnRef,
    owner: Option<&Binding>,
) -> Option<String> {
    match column {
        ColumnRef::Catalog { index } => {
            let entry = catalog.object(*index)?;
            let CatalogEntry::Column { parent, .. } = entry else {
                return None;
            };
            Some(catalog_column_card(catalog, *parent, entry))
        }
        ColumnRef::Local { name, data_type } => {
            let source = owner
                .and_then(|b| binding_owner(catalog, b))
                .unwrap_or_else(|| "query".to_string());
            let mut lines = vec![format!("column `{name}`")];
            lines.push(format!(
                "type: {}",
                data_type.as_deref().unwrap_or("unknown")
            ));
            lines.push(format!("from: {source}"));
            Some(lines.join("\n"))
        }
    }
}

fn catalog_column_card(catalog: &CatalogCache, relation: u32, column: &CatalogEntry) -> String {
    let CatalogEntry::Column {
        name,
        data_type,
        nullable,
        default,
        ..
    } = column
    else {
        return String::new();
    };
    let owner = catalog
        .object(relation)
        .map(|entry| format!("{}.{}", entry.schema().unwrap_or("public"), entry.name()))
        .unwrap_or_else(|| "?".to_string());
    let mut lines = vec![format!("column `{owner}.{name}`")];
    lines.push(format!(
        "type: {data_type}{}",
        if *nullable { "" } else { " not null" }
    ));
    if let Some(default) = default {
        lines.push(format!("default: {default}"));
    }
    lines.join("\n")
}

fn relation_card(catalog: &CatalogCache, relation: u32) -> Option<String> {
    let entry = catalog.object(relation)?;
    let header = match entry {
        CatalogEntry::Table {
            schema,
            name,
            estimated_rows,
            ..
        } => format!(
            "table {schema}.{name}{}",
            estimated_rows.map_or_else(String::new, |n| format!(" · ~{n} rows"))
        ),
        CatalogEntry::View {
            schema,
            name,
            definition,
            ..
        } => format!("view {schema}.{name}\ndefinition:\n{}", definition.trim()),
        _ => return None,
    };
    let mut lines = vec![header];
    let columns = catalog.columns_of(relation);
    if !columns.is_empty() {
        lines.push("columns:".to_string());
        for column in columns.iter().take(12) {
            if let CatalogEntry::Column {
                name,
                data_type,
                nullable,
                ..
            } = column
            {
                lines.push(format!(
                    "  {name} {data_type}{}",
                    if *nullable { "" } else { " not null" }
                ));
            }
        }
        if columns.len() > 12 {
            lines.push(format!("  … {} more", columns.len() - 12));
        }
    }
    Some(lines.join("\n"))
}

fn binding_card(catalog: &CatalogCache, binding: &Binding) -> String {
    let mut header = match (&binding.alias, &binding.name) {
        (Some(alias), Some(name)) => format!("alias `{alias}` → {name}"),
        (Some(alias), None) => format!("alias `{alias}`"),
        (None, Some(name)) => format!("`{name}`"),
        (None, None) => "source".to_string(),
    };
    if let Some(index) = binding.object
        && let Some(entry) = catalog.object(index)
    {
        header = match &binding.alias {
            Some(alias) => format!(
                "alias `{alias}` → {} {}.{}",
                kind_word(entry),
                entry.schema().unwrap_or("public"),
                entry.name()
            ),
            None => format!(
                "{} {}.{}",
                kind_word(entry),
                entry.schema().unwrap_or("public"),
                entry.name()
            ),
        };
    }
    let mut lines = vec![header];
    if !binding.columns.is_empty() {
        lines.push("columns:".to_string());
        for column in binding.columns.iter().take(12) {
            let name = column_name(catalog, column).unwrap_or_default();
            let ty = match column {
                ColumnRef::Catalog { index } => catalog.object(*index).and_then(|e| match e {
                    CatalogEntry::Column {
                        data_type,
                        nullable,
                        ..
                    } => Some(format!(
                        "{data_type}{}",
                        if *nullable { "" } else { " not null" }
                    )),
                    _ => None,
                }),
                ColumnRef::Local { data_type, .. } => data_type.as_ref().map(|t| t.to_string()),
            };
            lines.push(format!("  {name} {}", ty.unwrap_or_default()));
        }
        if binding.columns.len() > 12 {
            lines.push(format!("  … {} more", binding.columns.len() - 12));
        }
    }
    lines.join("\n")
}

fn function_card(entry: &CatalogEntry) -> String {
    let CatalogEntry::Function {
        schema,
        name,
        parameters,
        return_type,
        language,
        ..
    } = entry
    else {
        return String::new();
    };
    let params = parameters
        .iter()
        .map(|(name, ty)| format!("{name} {ty}"))
        .collect::<Vec<_>>()
        .join(", ");
    format!("function {schema}.{name}({params}) → {return_type}\nlanguage: {language}")
}

fn schema_card(catalog: &CatalogCache, schema: &str) -> String {
    let relations = catalog.relations_in_schema(schema);
    let mut names: Vec<String> = relations
        .iter()
        .filter_map(|idx| catalog.object(*idx))
        .map(|entry| entry.name().to_string())
        .collect();
    let total = names.len();
    names.truncate(20);
    let mut contents = format!("schema `{schema}` · {total} relations");
    if !names.is_empty() {
        contents.push('\n');
        contents.push_str(&names.join(", "));
        if total > names.len() {
            contents.push_str(", …");
        }
    }
    contents
}

fn kind_word(entry: &CatalogEntry) -> &'static str {
    match entry {
        CatalogEntry::Table { .. } => "table",
        CatalogEntry::View { .. } => "view",
        CatalogEntry::Column { .. } => "column",
        CatalogEntry::Index { .. } => "index",
        CatalogEntry::Function { .. } => "function",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::semantic::SemanticEngine;
    use std::sync::Arc;
    use tempr_domain::ids::{ConnectionId, SchemaObjectId, SchemaSnapshotId, SqlFileId};
    use tempr_domain::schema::{SchemaObject, SchemaSnapshot};
    use tempr_editor::Buffer;

    fn catalog() -> Arc<CatalogCache> {
        let users = SchemaObjectId::new();
        let orders = SchemaObjectId::new();
        let active = SchemaObjectId::new();
        let mut objects = vec![
            SchemaObject::Table {
                id: users,
                schema: "public".into(),
                name: "users".into(),
                estimated_rows: Some(42),
            },
            SchemaObject::Table {
                id: orders,
                schema: "public".into(),
                name: "orders".into(),
                estimated_rows: None,
            },
            SchemaObject::View {
                id: active,
                schema: "public".into(),
                name: "active_users".into(),
                definition: " SELECT users.id FROM users WHERE users.active;".into(),
            },
            SchemaObject::Function {
                id: SchemaObjectId::new(),
                schema: "public".into(),
                name: "total_sales".into(),
                parameters: vec![("region".into(), "text".into())],
                return_type: "numeric".into(),
                language: "sql".into(),
            },
        ];
        for (parent, name, ty, nullable, ordinal, default) in [
            (
                users,
                "id",
                "int8",
                false,
                0,
                Some("nextval('users_id_seq')"),
            ),
            (users, "email", "text", true, 1, None),
            (users, "name", "text", true, 2, None),
            (orders, "id", "int8", false, 0, None),
            (orders, "user_id", "int8", false, 1, None),
            (orders, "total", "numeric", true, 2, None),
        ] {
            objects.push(SchemaObject::Column {
                id: SchemaObjectId::new(),
                parent_id: parent,
                name: name.into(),
                data_type: ty.into(),
                nullable,
                ordinal,
                default: default.map(Into::into),
            });
        }
        for column in ["id", "email", "name", "active"] {
            objects.push(SchemaObject::Column {
                id: SchemaObjectId::new(),
                parent_id: active,
                name: column.into(),
                data_type: "text".into(),
                nullable: true,
                ordinal: 0,
                default: None,
            });
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
        Arc::new(CatalogCache::from_snapshot(&SchemaSnapshot {
            id: SchemaSnapshotId::new(),
            connection_id: ConnectionId::new(),
            version: 1,
            fetched_at: chrono::Utc::now(),
            objects: vec![],
            keywords: vec![],
            fingerprints: vec![],
        }))
    }

    fn hover_at(sql: &str, needle: &str, catalog: Arc<CatalogCache>) -> Option<HoverInfo> {
        let offset = sql.find(needle).expect("needle in sql") + needle.len();
        hover_at_offset(sql, offset, catalog)
    }

    fn hover_at_offset(sql: &str, offset: usize, catalog: Arc<CatalogCache>) -> Option<HoverInfo> {
        let engine = SemanticEngine::new(catalog);
        let mut buffer = Buffer::new(SqlFileId::new(), sql);
        engine.hover(buffer.file_id(), &mut buffer, offset)
    }

    #[test]
    fn qualified_column_shows_type_nullability_default_and_owner() {
        let sql = "select u.name from public.users u where u.id = 1";
        let hover = hover_at(sql, "u.id", catalog()).expect("hover");
        assert!(
            hover.contents.contains("column `public.users.id`"),
            "{}",
            hover.contents
        );
        assert!(
            hover.contents.contains("int8 not null"),
            "{}",
            hover.contents
        );
        assert!(
            hover.contents.contains("default: nextval('users_id_seq')"),
            "{}",
            hover.contents
        );
        assert_eq!(&sql[hover.range.clone()], "u.id");
    }

    #[test]
    fn bare_unique_column_resolves_through_the_scope() {
        let hover = hover_at("select email from users", "email", catalog()).expect("hover");
        assert!(
            hover.contents.contains("public.users.email"),
            "{}",
            hover.contents
        );
        assert!(hover.contents.contains("text"), "{}", hover.contents);
    }

    #[test]
    fn cursor_inside_a_word_expands_both_ways() {
        let sql = "select email from users";
        let offset = sql.find("email").expect("word") + 2;
        let hover = hover_at_offset(sql, offset, catalog()).expect("hover");
        assert!(
            hover.contents.contains("public.users.email"),
            "{}",
            hover.contents
        );
        assert_eq!(&sql[hover.range.clone()], "email");
    }

    #[test]
    fn relation_hover_lists_columns_and_rows() {
        let hover = hover_at("select * from users", "users", catalog()).expect("hover");
        assert!(
            hover.contents.contains("table public.users"),
            "{}",
            hover.contents
        );
        assert!(hover.contents.contains("~42 rows"), "{}", hover.contents);
        assert!(
            hover.contents.contains("id int8 not null"),
            "{}",
            hover.contents
        );
    }

    #[test]
    fn view_hover_includes_the_definition() {
        let hover =
            hover_at("select * from active_users", "active_users", catalog()).expect("hover");
        assert!(
            hover.contents.contains("view public.active_users"),
            "{}",
            hover.contents
        );
        assert!(
            hover.contents.contains("SELECT users.id"),
            "{}",
            hover.contents
        );
    }

    #[test]
    fn function_hover_shows_the_signature() {
        let hover = hover_at("select total_sales('eu')", "total_sales", catalog()).expect("hover");
        assert!(
            hover
                .contents
                .contains("function public.total_sales(region text) → numeric"),
            "{}",
            hover.contents
        );
    }

    #[test]
    fn schema_hover_lists_its_relations() {
        let hover = hover_at("select * from public.users", "public", catalog()).expect("hover");
        assert!(
            hover.contents.contains("schema `public`"),
            "{}",
            hover.contents
        );
        assert!(hover.contents.contains("users"), "{}", hover.contents);
    }

    #[test]
    fn cte_column_hover_uses_the_local_type() {
        let hover = hover_at(
            "with recent as (select id from users) select id from recent",
            "recent",
            catalog(),
        );
        let hover = hover.expect("hover");
        assert!(hover.contents.contains("`recent`"), "{}", hover.contents);
    }

    #[test]
    fn ambiguous_bare_column_says_so() {
        let hover = hover_at(
            "select id from users join orders on orders.user_id = users.id",
            "orders.user_id",
            catalog(),
        )
        .expect("hover");
        assert!(
            hover.contents.contains("orders.user_id"),
            "{}",
            hover.contents
        );
    }

    #[test]
    fn bare_column_ambiguity_is_scoped_to_the_innermost_nesting_level() {
        let sql = "select u.id from users u where exists (select id from orders o)";
        let offset = sql.find("select id from orders").expect("inner select") + "select id".len();
        let hover = hover_at_offset(sql, offset, catalog()).expect("hover");
        assert!(
            hover.contents.contains("public.orders.id"),
            "{}",
            hover.contents
        );
        assert!(!hover.contents.contains("ambiguous"), "{}", hover.contents);
    }

    #[test]
    fn a_database_prefix_is_ignored_when_qualifying_the_relation() {
        let sql = "select 1 from public.users where catalog.public.users.id = 1";
        let offset = sql.rfind("users.id").expect("needle") + "users.id".len();
        let hover = hover_at_offset(sql, offset, catalog()).expect("hover");
        assert!(
            hover.contents.contains("column `public.users.id`"),
            "{}",
            hover.contents
        );
    }

    #[test]
    fn offsets_inside_a_multibyte_character_have_no_reference() {
        let sql = "select é";
        let mid = sql.find('é').expect("é") + 1;
        assert!(
            !sql.is_char_boundary(mid),
            "test needs a mid-character byte"
        );
        assert!(reference_at(sql, mid).is_none());
    }

    #[test]
    fn no_catalog_and_unknown_symbols_have_no_card() {
        assert!(hover_at("select email from users", "email", empty_catalog()).is_none());
        let sql = "select email from users";
        let offset = sql.find(" from").expect("space") + 1;
        assert!(hover_at_offset(sql, offset, catalog()).is_none());
        assert!(hover_at_offset("select 1 + 2", 7, catalog()).is_none());
    }

    #[test]
    fn quoted_identifiers_are_unquoted_for_resolution() {
        let hover = hover_at("select \"name\" from users", "\"name\"", catalog()).expect("hover");
        assert!(
            hover.contents.contains("public.users.name"),
            "{}",
            hover.contents
        );
        assert_eq!(
            &"select \"name\" from users"[hover.range.clone()],
            "\"name\""
        );
    }
}
