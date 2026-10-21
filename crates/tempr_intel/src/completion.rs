//! Completion (spec §5.2): context classification, candidate generation,
//! ranking, and the bounded top-N result.
//!
//! The hot path reads only the stage-4 `Analysis` (cached per buffer revision)
//! and the immutable `CatalogCache` — no I/O, no DB queries, no tree walk
//! (ADR-0009). Context comes from a small lexer over the current statement's
//! text, not from tree-sitter nodes: the stage-5 probe found that a trailing
//! incomplete clause parses into a top-level `ERROR` sibling *outside* the
//! statement (`select * from ` → `(statement …) (ERROR (keyword_from))`), so
//! tree shape is unreliable exactly while the user types. The tree is still
//! what built the analysis; completion only consumes its scopes.
//!
//! Candidates are scored with one integer — context weight, then match
//! quality (exact prefix > folded prefix > fuzzy subsequence), then shorter
//! label — deduped by `(label, kind)` and kept in a bounded 200-item heap
//! (spec §5.2). Only returned items own strings.

use std::cmp::Ordering;
use std::collections::BinaryHeap;
use std::collections::HashMap;
use std::collections::HashSet;
use std::collections::hash_map::Entry;
use std::ops::Range;

use tempr_domain::SqlFileId;
use tempr_domain::text::fuzzy_match;
use tempr_editor::Buffer;

use crate::analysis::{Analysis, Binding, ColumnRef, Scope};
use crate::catalog::{CatalogCache, CatalogEntry};
use crate::semantic::SemanticEngine;

/// Hard cap on returned items (spec §5.2's bounded top-N).
pub const COMPLETION_LIMIT: usize = 200;

/// Catalog prefix scans stop here; ranking keeps at most `COMPLETION_LIMIT`.
const PREFIX_SCAN_CAP: usize = 200;

// Context weights: columns beat relations, relations beat functions, and
// keywords are the floor (spec §5.2 "columns in scope weighted up").
const WEIGHT_COLUMN: i32 = 500;
const WEIGHT_COLUMN_QUALIFIED: i32 = 480;
const WEIGHT_CTE: i32 = 470;
const WEIGHT_RELATION: i32 = 450;
const WEIGHT_STATEMENT_KEYWORD: i32 = 400;
const WEIGHT_FUNCTION: i32 = 300;
const WEIGHT_RELATION_EXPRESSION: i32 = 250;
const WEIGHT_SCHEMA: i32 = 200;
const WEIGHT_ALIAS: i32 = 150;
const WEIGHT_KEYWORD: i32 = 100;

/// Keywords offered when the catalog carries no server keyword list (no
/// connection yet, or a driver that does not implement `keywords()`).
const FALLBACK_KEYWORDS: &[&str] = &[
    "select",
    "from",
    "where",
    "and",
    "or",
    "not",
    "null",
    "is",
    "in",
    "like",
    "ilike",
    "between",
    "exists",
    "join",
    "inner",
    "left",
    "right",
    "full",
    "outer",
    "cross",
    "on",
    "using",
    "as",
    "distinct",
    "group",
    "by",
    "order",
    "having",
    "limit",
    "offset",
    "union",
    "all",
    "intersect",
    "except",
    "case",
    "when",
    "then",
    "else",
    "end",
    "insert",
    "into",
    "values",
    "update",
    "set",
    "delete",
    "returning",
    "with",
    "recursive",
    "create",
    "alter",
    "drop",
    "table",
    "view",
    "index",
    "schema",
    "function",
    "begin",
    "commit",
    "rollback",
    "explain",
    "analyze",
    "count",
    "sum",
    "avg",
    "min",
    "max",
    "coalesce",
    "cast",
    "now",
    "current_date",
    "current_timestamp",
    "true",
    "false",
    "asc",
    "desc",
    "nulls",
    "first",
    "last",
    "over",
    "partition",
    "window",
    "lateral",
    "fetch",
    "for",
    "of",
    "only",
];

/// Statement starters (spec §5.2 "statement start → statement keywords").
/// Static rather than server-derived: `pg_get_keywords()` is unordered and
/// includes non-statement words, and this list changes only with SQL itself.
const STATEMENT_KEYWORDS: &[&str] = &[
    "SELECT",
    "INSERT",
    "UPDATE",
    "DELETE",
    "WITH",
    "CREATE",
    "ALTER",
    "DROP",
    "TRUNCATE",
    "EXPLAIN",
    "ANALYZE",
    "VACUUM",
    "REINDEX",
    "CLUSTER",
    "REFRESH",
    "COMMENT",
    "GRANT",
    "REVOKE",
    "COPY",
    "BEGIN",
    "START",
    "COMMIT",
    "ROLLBACK",
    "SAVEPOINT",
    "SET",
    "SHOW",
    "RESET",
    "LISTEN",
    "NOTIFY",
    "UNLISTEN",
    "PREPARE",
    "EXECUTE",
    "DEALLOCATE",
    "LOCK",
    "CALL",
    "DO",
    "DECLARE",
    "FETCH",
    "MOVE",
    "CLOSE",
];

/// One completion request. `buffer` is `&mut` because the analysis cache
/// lazily reparses the syntax tree (the stage-4 deviation from the spec's
/// `&BufferSnapshot` sketch); completion itself only reads text and analysis.
#[derive(Debug)]
pub struct CompletionRequest<'a> {
    pub file: SqlFileId,
    pub buffer: &'a mut Buffer,
    /// Byte offset of the cursor.
    pub offset: usize,
    /// True for `ctrl-space`: bypasses the auto-open gating.
    pub explicit: bool,
}

/// Category for the popup's icon/grouping (docs/12-sql-intelligence.md).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum CompletionKind {
    Keyword,
    Table,
    Column,
    Schema,
    Alias,
    Cte,
    Function,
    Snippet,
    Plugin,
}

/// One ranked candidate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletionItem {
    /// Text shown in the popup.
    pub label: String,
    pub kind: CompletionKind,
    /// Type, signature, schema or ownership hint (empty when there is none).
    pub detail: String,
    /// Text the editor writes. Equal to `label` today; separate so stage-6
    /// snippets can differ without reshaping the item.
    pub insert: String,
    /// Byte range in the buffer the item replaces (the word under the
    /// cursor; empty when the cursor follows a dot or whitespace).
    pub replace: Range<usize>,
    /// Ranking score, highest first.
    pub score: i32,
}

/// What the cursor position expects (spec §5.2's table).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompletionContext {
    /// Nothing before the cursor, or after a complete `WITH` clause.
    StatementStart,
    /// After `FROM`/`JOIN`/`UPDATE`/`INTO`/`TABLE`, or after a comma inside
    /// one of those clauses.
    Relation,
    /// Any expression position (select list, predicates, arguments).
    Expression,
    /// After `AS`/`WITH`: the user names the alias/CTE — no candidates.
    Alias,
    /// After a qualifier dot: `u.` / `public.` / `public.users.`.
    Qualifier(Vec<String>),
}

/// The classified cursor plus the word it replaces.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CursorContext {
    pub context: CompletionContext,
    pub word: Range<usize>,
    pub prefix: String,
}

impl SemanticEngine {
    /// Ranked completion candidates at `req.offset` (spec §5.2). Never
    /// mutates the buffer beyond the lazy reparse the analysis cache needs.
    pub fn complete(&self, req: CompletionRequest<'_>) -> Vec<CompletionItem> {
        let CompletionRequest {
            file,
            buffer,
            offset,
            explicit,
        } = req;
        let offset = offset.min(buffer.len());
        let analysis = self.analyze(file, buffer);
        let catalog = self.catalog();
        complete_with(&catalog, &analysis, buffer, offset, explicit)
    }
}

fn complete_with(
    catalog: &CatalogCache,
    analysis: &Analysis,
    buffer: &Buffer,
    offset: usize,
    explicit: bool,
) -> Vec<CompletionItem> {
    let scopes = frame_chain(analysis, buffer, offset);
    let statement_start = analysis
        .scopes
        .iter()
        .rev()
        .find(|scope| scope.range.start <= offset)
        .map_or(0, |scope| scope.range.start);
    let prefix_text = buffer.slice(statement_start..offset).unwrap_or_default();
    let cursor = cursor_context(&prefix_text, statement_start);

    // Auto-open rule (spec decision 5): one identifier character or a
    // qualifier dot opens the popup; otherwise only ctrl-space does.
    if !explicit
        && cursor.prefix.is_empty()
        && !matches!(cursor.context, CompletionContext::Qualifier(_))
    {
        return Vec::new();
    }
    if matches!(cursor.context, CompletionContext::Alias) {
        return Vec::new();
    }

    let generator = Generator {
        catalog,
        loaded: !catalog.is_empty(),
        scopes,
    };
    let candidates = generator.generate(&cursor);
    rank(candidates, &cursor.prefix, cursor.word)
}

/// Scopes visible at the cursor, innermost first. A trailing incomplete
/// clause leaves the statement as a top-level `ERROR` sibling (probe
/// finding), so an empty chain falls back to the statement it trails —
/// otherwise `select * from users u where ` would lose its bindings. The
/// fallback stops at a statement separator: an `ERROR`-only statement (say
/// `select * frm ` after a `;`) must not borrow the previous statement's
/// bindings.
fn frame_chain<'a>(analysis: &'a Analysis, buffer: &Buffer, offset: usize) -> Vec<&'a Scope> {
    let mut chain = analysis.scope_chain(offset);
    chain.reverse();
    if !chain
        .iter()
        .all(|s| s.bindings.is_empty() && s.ctes.is_empty())
    {
        return chain;
    }
    match analysis
        .scopes
        .iter()
        .rev()
        .find(|scope| scope.range.start <= offset)
    {
        Some(scope) if !trails_a_statement(buffer, scope.range.end, offset) => vec![scope],
        _ => chain,
    }
}

/// Does the text between the candidate scope's end and the cursor contain a
/// statement separator? Comments and strings are not special-cased: a `;`
/// there still means the fragment is a new statement in every realistic
/// substitute for this test.
fn trails_a_statement(buffer: &Buffer, end: usize, offset: usize) -> bool {
    let end = end.min(offset).min(buffer.len());
    buffer
        .slice(end..offset)
        .map_or(true, |text| text.contains(';'))
}

// === Context classification ===

/// Classify the text from the statement start to the cursor.
fn cursor_context(prefix: &str, base: usize) -> CursorContext {
    let mut word_start = word_start(prefix);
    // An unterminated quoted identifier: include the opening quote in the
    // replacement range, so accepting `users` replaces `"us` instead of
    // leaving a dangling quote. Matching uses the text after the quote.
    let mut typed_start = word_start;
    if let Some(quote) = open_quote_before(prefix, word_start) {
        word_start = quote;
        typed_start = quote + 1;
    }
    let qualifier = qualifier_before(prefix, word_start);
    let typed = &prefix[typed_start..];
    let context = match &qualifier {
        Some((parts, _)) => CompletionContext::Qualifier(parts.clone()),
        None => classify(&tokenize(&prefix[..word_start])),
    };
    CursorContext {
        context,
        word: base + word_start..base + prefix.len(),
        prefix: typed.to_string(),
    }
}

/// Index of the `"` that opens an unterminated quoted identifier ending at
/// `word_start`, if the cursor is inside one. Doubled (`""`) quotes are
/// escapes and do not change the parity.
fn open_quote_before(text: &str, word_start: usize) -> Option<usize> {
    let prefix = &text[..word_start];
    if !prefix.ends_with('"') {
        return None;
    }
    let bytes = prefix.as_bytes();
    let mut quotes = 0usize;
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'"' {
            if i + 1 < bytes.len() && bytes[i + 1] == b'"' {
                i += 2;
                continue;
            }
            quotes += 1;
        }
        i += 1;
    }
    (quotes % 2 == 1).then_some(word_start - 1)
}

fn is_ident_start(c: char) -> bool {
    c.is_alphabetic() || c == '_'
}

fn is_ident_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '$'
}

/// Start of the identifier being typed (the replacement word).
fn word_start(text: &str) -> usize {
    let mut start = text.len();
    for (idx, c) in text.char_indices().rev() {
        if is_ident_char(c) {
            start = idx;
        } else {
            break;
        }
    }
    start
}

/// The qualifier path before a dot, left to right, plus the byte offset
/// where it begins. Whitespace around dots is tolerated (`u . id` is valid
/// PostgreSQL); quoted parts are unquoted for matching.
fn qualifier_before(text: &str, word_start: usize) -> Option<(Vec<String>, usize)> {
    let mut cursor = word_start;
    let mut parts = Vec::new();
    loop {
        let before = text[..cursor].trim_end();
        if !before.ends_with('.') {
            break;
        }
        let dot = before.len() - 1;
        let Some((part, start)) = identifier_back(&text[..dot]) else {
            break;
        };
        parts.push(part);
        cursor = start;
        if parts.len() == 3 {
            break;
        }
    }
    if parts.is_empty() {
        return None;
    }
    parts.reverse();
    Some((parts, cursor))
}

/// Read the identifier ending `text` (already trimmed at the right).
fn identifier_back(text: &str) -> Option<(String, usize)> {
    let text = text.trim_end();
    if let Some(body) = text.strip_suffix('"') {
        // Quoted identifier: find the opening quote, honouring "" escapes.
        let bytes = body.as_bytes();
        let mut i = bytes.len();
        while i > 0 {
            i -= 1;
            if bytes[i] == b'"' {
                let escaped = i > 0 && bytes[i - 1] == b'"';
                if escaped {
                    i -= 1;
                    continue;
                }
                return Some((body[i + 1..].replace("\"\"", "\""), i));
            }
        }
        return None;
    }
    let mut start = text.len();
    for (idx, c) in text.char_indices().rev() {
        if is_ident_char(c) {
            start = idx;
        } else {
            break;
        }
    }
    let ident = &text[start..];
    if ident.is_empty() || !ident.chars().next().is_some_and(is_ident_start) {
        return None;
    }
    Some((ident.to_string(), start))
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Token {
    /// Unquoted word or number; classification folds keywords only.
    Word(String),
    /// Quoted identifier: never a clause keyword.
    Quoted,
    Dot,
    Comma,
    Open,
    Close,
    Other,
}

fn tokenize(mut rest: &str) -> Vec<Token> {
    let mut tokens = Vec::new();
    loop {
        rest = rest.trim_start();
        let Some(first) = rest.chars().next() else {
            break;
        };
        if rest.starts_with("--") {
            match rest.find('\n') {
                Some(end) => rest = &rest[end + 1..],
                None => break,
            }
            continue;
        }
        if rest.starts_with("/*") {
            rest = skip_block_comment(rest);
            continue;
        }
        match first {
            '\'' => rest = skip_single_quoted(rest),
            '"' => {
                let (_, after) = take_quoted(rest);
                tokens.push(Token::Quoted);
                rest = after;
            }
            '$' => match skip_dollar_quoted(rest) {
                Some(after) => rest = after,
                None => {
                    let (word, after) = take_word(rest);
                    tokens.push(Token::Word(word));
                    rest = after;
                }
            },
            '.' => {
                tokens.push(Token::Dot);
                rest = &rest[1..];
            }
            ',' => {
                tokens.push(Token::Comma);
                rest = &rest[1..];
            }
            '(' => {
                tokens.push(Token::Open);
                rest = &rest[1..];
            }
            ')' => {
                tokens.push(Token::Close);
                rest = &rest[1..];
            }
            c if is_ident_start(c) => {
                let (word, after) = take_word(rest);
                tokens.push(Token::Word(word));
                rest = after;
            }
            c if c.is_ascii_digit() => {
                let (word, after) = take_word(rest);
                tokens.push(Token::Word(word));
                rest = after;
            }
            c => {
                tokens.push(Token::Other);
                rest = &rest[c.len_utf8()..];
            }
        }
    }
    tokens
}

fn take_word(text: &str) -> (String, &str) {
    let end = text
        .char_indices()
        .find(|(_, c)| !is_ident_char(*c))
        .map_or(text.len(), |(idx, _)| idx);
    (text[..end].to_string(), &text[end..])
}

fn skip_single_quoted(text: &str) -> &str {
    let mut chars = text.char_indices();
    chars.next(); // opening quote
    let mut escaped = false;
    for (idx, c) in chars {
        if escaped {
            escaped = false;
            continue;
        }
        if c == '\'' {
            // A doubled quote is an escape; consume it and keep going.
            let next = idx + 1;
            if text[next..].starts_with('\'') {
                escaped = true;
            } else {
                return &text[next..];
            }
        }
    }
    ""
}

fn skip_block_comment(text: &str) -> &str {
    let mut depth = 0usize;
    let mut idx = 0usize;
    while idx < text.len() {
        let rest = &text[idx..];
        if rest.starts_with("/*") {
            depth += 1;
            idx += 2;
        } else if rest.starts_with("*/") {
            depth = depth.saturating_sub(1);
            idx += 2;
            if depth == 0 {
                return &text[idx..];
            }
        } else if let Some(c) = rest.chars().next() {
            idx += c.len_utf8();
        } else {
            break;
        }
    }
    ""
}

fn take_quoted(text: &str) -> (String, &str) {
    let mut chars = text.char_indices();
    chars.next();
    let mut body = String::new();
    let mut escaped = false;
    for (idx, c) in chars {
        if escaped {
            escaped = false;
            body.push(c);
            continue;
        }
        match c {
            '"' => {
                let next = idx + 1;
                if text[next..].starts_with('"') {
                    escaped = true;
                } else {
                    return (body, &text[next..]);
                }
            }
            _ => body.push(c),
        }
    }
    (body, "")
}

/// `$tag$ … $tag$`; `None` when the `$` starts an identifier/parameter
/// instead of a dollar quote.
fn skip_dollar_quoted(text: &str) -> Option<&str> {
    let rest = text.strip_prefix('$')?;
    let tag_end = rest
        .char_indices()
        .find(|(_, c)| !(c.is_alphanumeric() || *c == '_'))
        .map_or(rest.len(), |(idx, _)| idx);
    if !rest[tag_end..].starts_with('$') {
        return None;
    }
    let tag = &text[..tag_end + 2];
    let body_start = tag_end + 2;
    text[body_start..]
        .find(tag)
        .map(|offset| &text[body_start + offset + tag.len()..])
        .or(Some(""))
}

fn classify(tokens: &[Token]) -> CompletionContext {
    let Some(last) = tokens.last() else {
        return CompletionContext::StatementStart;
    };
    match last {
        Token::Word(word) => match word.to_ascii_lowercase().as_str() {
            "as" | "with" => CompletionContext::Alias,
            "from" | "join" | "update" | "into" | "table" | "truncate" | "only" => {
                CompletionContext::Relation
            }
            _ => CompletionContext::Expression,
        },
        Token::Comma => {
            if paren_depth(tokens) > 0 {
                CompletionContext::Expression
            } else {
                match nearest_clause(tokens).as_deref() {
                    Some("from" | "join" | "update" | "into") => CompletionContext::Relation,
                    // `with a as (...), ` names the next CTE.
                    Some("with") => CompletionContext::Alias,
                    _ => CompletionContext::Expression,
                }
            }
        }
        Token::Close => {
            if paren_depth(tokens) == 0 && starts_with_with(tokens) {
                CompletionContext::StatementStart
            } else {
                CompletionContext::Expression
            }
        }
        _ => CompletionContext::Expression,
    }
}

/// Did this statement begin with `WITH` (so a completed CTE clause puts the
/// cursor back at a statement start)?
fn starts_with_with(tokens: &[Token]) -> bool {
    matches!(
        tokens.first(),
        Some(Token::Word(word)) if word.eq_ignore_ascii_case("with")
    )
}

fn paren_depth(tokens: &[Token]) -> usize {
    let mut depth = 0usize;
    for token in tokens {
        match token {
            Token::Open => depth += 1,
            Token::Close => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    depth
}

/// The last clause keyword at paren depth 0, scanning backwards. Keywords
/// inside a subquery's parentheses belong to that subquery, not to the
/// clause the cursor is in.
fn nearest_clause(tokens: &[Token]) -> Option<String> {
    const CLAUSES: &[&str] = &[
        "select",
        "from",
        "where",
        "join",
        "on",
        "using",
        "group",
        "order",
        "having",
        "limit",
        "offset",
        "returning",
        "set",
        "values",
        "into",
        "update",
        "insert",
        "with",
        "union",
        "intersect",
        "except",
        "window",
        "partition",
        "fetch",
        "for",
    ];
    let mut depth = 0usize;
    for token in tokens.iter().rev() {
        match token {
            Token::Close => depth += 1,
            Token::Open => depth = depth.saturating_sub(1),
            Token::Word(word) if depth == 0 => {
                let folded = word.to_ascii_lowercase();
                if CLAUSES.contains(&folded.as_str()) {
                    return Some(folded);
                }
            }
            _ => {}
        }
    }
    None
}

// === Candidate generation ===

struct Candidate {
    label: String,
    insert: String,
    kind: CompletionKind,
    detail: String,
    weight: i32,
}

struct Generator<'a> {
    catalog: &'a CatalogCache,
    loaded: bool,
    scopes: Vec<&'a Scope>,
}

impl Generator<'_> {
    fn generate(&self, cursor: &CursorContext) -> Vec<Candidate> {
        let mut out = Vec::new();
        // Spec §7 row 1: with no catalog, completion offers keywords only.
        if !self.loaded {
            self.keywords(&mut out);
            return out;
        }
        match &cursor.context {
            CompletionContext::StatementStart => self.statement_keywords(&mut out),
            CompletionContext::Relation => {
                self.ctes(&mut out);
                self.relations(&cursor.prefix, &mut out, WEIGHT_RELATION);
                self.schemas(&mut out);
            }
            CompletionContext::Expression => {
                self.columns(&mut out);
                self.relations(&cursor.prefix, &mut out, WEIGHT_RELATION_EXPRESSION);
                self.functions(&cursor.prefix, &mut out);
                self.keywords(&mut out);
                self.aliases(&mut out);
            }
            CompletionContext::Alias => {}
            CompletionContext::Qualifier(parts) => self.qualifier(parts, &mut out),
        }
        out
    }

    fn visible_bindings(&self) -> impl Iterator<Item = &Binding> {
        self.scopes.iter().flat_map(|scope| scope.bindings.iter())
    }

    fn visible_ctes(&self) -> impl Iterator<Item = &Binding> {
        self.scopes.iter().flat_map(|scope| scope.ctes.iter())
    }

    /// A source by qualifier: each scope's CTEs shadow its bindings, and the
    /// innermost scope wins (same rule the analyzer applies).
    fn qualifier_binding(&self, qualifier: &str) -> Option<&Binding> {
        for scope in &self.scopes {
            if let Some(binding) = scope.ctes.iter().find(|b| b.matches_qualifier(qualifier)) {
                return Some(binding);
            }
            if let Some(binding) = scope
                .bindings
                .iter()
                .find(|b| b.matches_qualifier(qualifier))
            {
                return Some(binding);
            }
        }
        None
    }

    fn ctes(&self, out: &mut Vec<Candidate>) {
        for cte in self.visible_ctes() {
            let Some(name) = cte.name.as_deref() else {
                continue;
            };
            out.push(Candidate {
                label: name.to_string(),
                insert: name.to_string(),
                kind: CompletionKind::Cte,
                detail: format!("cte · {} columns", cte.columns.len()),
                weight: WEIGHT_CTE,
            });
        }
    }

    fn relations(&self, prefix: &str, out: &mut Vec<Candidate>, weight: i32) {
        let (indices, _) = self.catalog.prefix_search(prefix, PREFIX_SCAN_CAP);
        for index in indices {
            let Some(entry) = self.catalog.object(index) else {
                continue;
            };
            let (kind_word, schema) = match entry {
                CatalogEntry::Table { schema, .. } => ("table", schema),
                CatalogEntry::View { schema, .. } => ("view", schema),
                _ => continue,
            };
            out.push(Candidate {
                label: entry.name().to_string(),
                insert: entry.name().to_string(),
                kind: CompletionKind::Table,
                detail: format!("{kind_word} · {schema}"),
                weight,
            });
        }
    }

    fn functions(&self, prefix: &str, out: &mut Vec<Candidate>) {
        let (indices, _) = self.catalog.prefix_search(prefix, PREFIX_SCAN_CAP);
        for index in indices {
            let Some(CatalogEntry::Function {
                name,
                parameters,
                return_type,
                ..
            }) = self.catalog.object(index)
            else {
                continue;
            };
            let args = parameters
                .iter()
                .map(|(arg, ty)| format!("{arg} {ty}"))
                .collect::<Vec<_>>()
                .join(", ");
            out.push(Candidate {
                label: name.to_string(),
                insert: name.to_string(),
                kind: CompletionKind::Function,
                detail: format!("({args}) → {return_type}"),
                weight: WEIGHT_FUNCTION,
            });
        }
    }

    fn schemas(&self, out: &mut Vec<Candidate>) {
        for schema in self.catalog.schemas() {
            out.push(Candidate {
                label: schema.to_string(),
                insert: schema.to_string(),
                kind: CompletionKind::Schema,
                detail: "schema".to_string(),
                weight: WEIGHT_SCHEMA,
            });
        }
    }

    fn keywords(&self, out: &mut Vec<Candidate>) {
        if self.catalog.keywords().is_empty() {
            for keyword in FALLBACK_KEYWORDS {
                push_keyword(out, keyword, WEIGHT_KEYWORD);
            }
            return;
        }
        for keyword in self.catalog.keywords() {
            push_keyword(out, keyword, WEIGHT_KEYWORD);
        }
    }

    fn statement_keywords(&self, out: &mut Vec<Candidate>) {
        for keyword in STATEMENT_KEYWORDS {
            push_keyword(out, keyword, WEIGHT_STATEMENT_KEYWORD);
        }
    }

    fn aliases(&self, out: &mut Vec<Candidate>) {
        for binding in self.visible_bindings() {
            let Some(alias) = binding.alias.as_deref() else {
                continue;
            };
            out.push(Candidate {
                label: alias.to_string(),
                insert: alias.to_string(),
                kind: CompletionKind::Alias,
                detail: "alias".to_string(),
                weight: WEIGHT_ALIAS,
            });
        }
    }

    /// Columns of every visible binding: bare when the folded name is
    /// unique, qualified (`alias.col`) when it repeats across bindings
    /// (spec §5.2 — ambiguity is resolved by the user, not guessed).
    fn columns(&self, out: &mut Vec<Candidate>) {
        let bindings: Vec<&Binding> = self.visible_bindings().collect();
        let mut counts: HashMap<String, usize> = HashMap::new();
        for binding in &bindings {
            // One binding exposing the same output name twice (legal in a
            // subquery) is not cross-binding ambiguity.
            let mut own: HashSet<String> = HashSet::new();
            for column in &binding.columns {
                if let Some((name, _)) = self.column_info(column) {
                    let key = name.to_lowercase();
                    if own.insert(key.clone()) {
                        *counts.entry(key).or_insert(0) += 1;
                    }
                }
            }
        }

        for binding in bindings {
            let owner = binding_qualifier(binding);
            for column in &binding.columns {
                let Some((name, ty)) = self.column_info(column) else {
                    continue;
                };
                let detail = column_detail(ty.as_deref(), owner);
                if counts.get(&name.to_lowercase()).copied().unwrap_or(0) <= 1 {
                    out.push(Candidate {
                        label: name.clone(),
                        insert: name,
                        kind: CompletionKind::Column,
                        detail,
                        weight: WEIGHT_COLUMN,
                    });
                } else if let Some(owner) = owner {
                    out.push(Candidate {
                        label: format!("{owner}.{name}"),
                        insert: format!("{owner}.{name}"),
                        kind: CompletionKind::Column,
                        detail,
                        weight: WEIGHT_COLUMN_QUALIFIED,
                    });
                }
            }
        }
    }

    fn qualifier(&self, parts: &[String], out: &mut Vec<Candidate>) {
        let Some(last) = parts.last() else {
            return;
        };

        // Three-part `schema.relation.` resolves through the catalog.
        if parts.len() >= 2 {
            let schema = &parts[parts.len() - 2];
            let mut relations = self.catalog.relation_folded(schema, last);
            if relations.is_empty()
                && let Some(exact) = self.catalog.relation_exact(schema, last)
            {
                relations.push(exact);
            }
            for index in relations {
                self.catalog_columns(index, Some(last), out);
            }
            return;
        }

        if let Some(binding) = self.qualifier_binding(last) {
            // An unknown relation binds with no column knowledge (the
            // analyzer's cascade-stopping rule); let a matching schema win
            // instead, so `select * from public.` completes relations.
            if !binding.columns.is_empty() || binding.object.is_some() {
                self.binding_columns(binding, out);
                return;
            }
        }

        // A schema dot lists that schema's relations.
        if self
            .catalog
            .schemas()
            .iter()
            .any(|schema| schema.to_lowercase() == last.to_lowercase())
        {
            for index in self.catalog.relations_in_schema(last) {
                let Some(entry) = self.catalog.object(index) else {
                    continue;
                };
                let kind_word = match entry {
                    CatalogEntry::View { .. } => "view",
                    _ => "table",
                };
                out.push(Candidate {
                    label: entry.name().to_string(),
                    insert: entry.name().to_string(),
                    kind: CompletionKind::Table,
                    detail: format!("{kind_word} · {last}"),
                    weight: WEIGHT_RELATION,
                });
            }
        }
    }

    fn binding_columns(&self, binding: &Binding, out: &mut Vec<Candidate>) {
        for column in &binding.columns {
            let Some((name, ty)) = self.column_info(column) else {
                continue;
            };
            let owner = binding_qualifier(binding);
            out.push(Candidate {
                label: name.clone(),
                insert: name,
                kind: CompletionKind::Column,
                detail: column_detail(ty.as_deref(), owner),
                weight: WEIGHT_COLUMN,
            });
        }
    }

    /// Columns of a catalog relation resolved by name (`schema.relation.`),
    /// which has no `Binding` in the analysis to read from.
    fn catalog_columns(&self, index: u32, owner: Option<&str>, out: &mut Vec<Candidate>) {
        for entry in self.catalog.columns_of(index) {
            let CatalogEntry::Column {
                name,
                data_type,
                nullable,
                ..
            } = entry
            else {
                continue;
            };
            let ty = if *nullable {
                data_type.to_string()
            } else {
                format!("{data_type} not null")
            };
            out.push(Candidate {
                label: name.to_string(),
                insert: name.to_string(),
                kind: CompletionKind::Column,
                detail: column_detail(Some(&ty), owner),
                weight: WEIGHT_COLUMN,
            });
        }
    }

    fn column_info(&self, column: &ColumnRef) -> Option<(String, Option<String>)> {
        match column {
            ColumnRef::Catalog { index } => match self.catalog.object(*index) {
                Some(CatalogEntry::Column {
                    name,
                    data_type,
                    nullable,
                    ..
                }) => Some((
                    name.to_string(),
                    Some(if *nullable {
                        data_type.to_string()
                    } else {
                        format!("{data_type} not null")
                    }),
                )),
                _ => None,
            },
            ColumnRef::Local { name, data_type } => {
                Some((name.to_string(), data_type.as_ref().map(|t| t.to_string())))
            }
        }
    }
}

fn push_keyword(out: &mut Vec<Candidate>, keyword: &str, weight: i32) {
    let label = keyword.to_uppercase();
    out.push(Candidate {
        label: label.clone(),
        insert: label,
        kind: CompletionKind::Keyword,
        detail: String::new(),
        weight,
    });
}

fn binding_qualifier(binding: &Binding) -> Option<&str> {
    binding.alias.as_deref().or(binding.name.as_deref())
}

fn column_detail(ty: Option<&str>, owner: Option<&str>) -> String {
    match (ty, owner) {
        (Some(ty), Some(owner)) => format!("{ty} · {owner}"),
        (Some(ty), None) => ty.to_string(),
        (None, Some(owner)) => owner.to_string(),
        (None, None) => String::new(),
    }
}

// === Ranking ===

/// One scored candidate; ordering is the final result order (score desc,
/// label asc) so the heap can evict the worst item.
struct Ranked {
    score: i32,
    label: String,
    candidate: Candidate,
}

impl PartialEq for Ranked {
    fn eq(&self, other: &Self) -> bool {
        self.score == other.score && self.label == other.label
    }
}

impl Eq for Ranked {}

impl Ord for Ranked {
    fn cmp(&self, other: &Self) -> Ordering {
        self.score
            .cmp(&other.score)
            .then_with(|| other.label.cmp(&self.label))
    }
}

impl PartialOrd for Ranked {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

fn match_quality(prefix: &str, folded_prefix: &str, label: &str) -> Option<i32> {
    if prefix.is_empty() {
        return Some(0);
    }
    if label.starts_with(prefix) {
        return Some(400);
    }
    if label.to_lowercase().starts_with(folded_prefix) {
        return Some(300);
    }
    fuzzy_match(prefix, label).map(|m| m.score)
}

fn rank(candidates: Vec<Candidate>, prefix: &str, replace: Range<usize>) -> Vec<CompletionItem> {
    let folded_prefix = prefix.to_lowercase();
    let mut best: HashMap<(String, CompletionKind), Ranked> = HashMap::new();
    for candidate in candidates {
        let Some(quality) = match_quality(prefix, &folded_prefix, &candidate.label) else {
            continue;
        };
        let score = candidate.weight * 1000 + quality * 4 - candidate.label.chars().count() as i32;
        let key = (candidate.label.clone(), candidate.kind);
        match best.entry(key) {
            Entry::Vacant(slot) => {
                slot.insert(Ranked {
                    score,
                    label: candidate.label.clone(),
                    candidate,
                });
            }
            Entry::Occupied(mut slot) => {
                if score > slot.get().score {
                    slot.insert(Ranked {
                        score,
                        label: candidate.label.clone(),
                        candidate,
                    });
                }
            }
        }
    }

    // Bounded top-N: `Reverse` puts the worst kept item on top of the heap.
    // `best` already deduped by (label, kind), so every entry is distinct.
    let mut heap: BinaryHeap<std::cmp::Reverse<Ranked>> = BinaryHeap::new();
    for ranked in best.into_values() {
        if heap.len() < COMPLETION_LIMIT {
            heap.push(std::cmp::Reverse(ranked));
        } else if let Some(std::cmp::Reverse(worst)) = heap.peek()
            && ranked > *worst
        {
            heap.pop();
            heap.push(std::cmp::Reverse(ranked));
        }
    }

    let mut items: Vec<Ranked> = heap
        .into_iter()
        .map(|std::cmp::Reverse(ranked)| ranked)
        .collect();
    items.sort_by(|a, b| b.cmp(a));
    items
        .into_iter()
        .map(|ranked| CompletionItem {
            label: ranked.candidate.label,
            kind: ranked.candidate.kind,
            detail: ranked.candidate.detail,
            insert: ranked.candidate.insert,
            replace: replace.clone(),
            score: ranked.score,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use self::tempr_intel_catalog_test_support::{catalog, empty_catalog};
    use super::*;
    use tempr_domain::ids::{ConnectionId, SchemaObjectId, SchemaSnapshotId};
    use tempr_domain::schema::{SchemaObject, SchemaSnapshot};

    fn context(sql: &str) -> CompletionContext {
        cursor_context(sql, 0).context
    }

    #[test]
    fn classifier_matches_the_probe_table() {
        assert_eq!(context(""), CompletionContext::StatementStart);
        assert_eq!(context("sel"), CompletionContext::StatementStart);
        assert_eq!(
            context("with c as (select 1) "),
            CompletionContext::StatementStart
        );

        for sql in [
            "select * from ",
            "select * from us",
            "select * from users u join ",
            "insert into ",
            "update ",
            "select * from users u, ",
            "select * from (select 1) x, ",
        ] {
            assert_eq!(context(sql), CompletionContext::Relation, "{sql:?}");
        }

        for sql in [
            "select ",
            "select * from users where ",
            "select * from users where name = 'x' ",
            "select 'from ' , ",
            "select count(",
            "-- from\nselect ",
            "/* from */ select ",
        ] {
            assert_eq!(context(sql), CompletionContext::Expression, "{sql:?}");
        }

        assert_eq!(context("select id as "), CompletionContext::Alias);
        assert_eq!(
            context("with c as (select 1), "),
            CompletionContext::Alias,
            "a top-level comma after a CTE names the next CTE"
        );
        assert_eq!(
            context("select u."),
            CompletionContext::Qualifier(vec!["u".into()])
        );
        assert_eq!(
            context("select public."),
            CompletionContext::Qualifier(vec!["public".into()])
        );
        assert_eq!(
            context("select * from users u where u."),
            CompletionContext::Qualifier(vec!["u".into()])
        );
        assert_eq!(
            context("select * from public.users u where public.users."),
            CompletionContext::Qualifier(vec!["public".into(), "users".into()])
        );
    }

    #[test]
    fn classifier_skips_strings_comments_and_dollar_quotes() {
        assert_eq!(
            context("select * from users where name = 'from' "),
            CompletionContext::Expression
        );
        assert_eq!(
            context("select $$from$$ , "),
            CompletionContext::Expression,
            "dollar-quoted body is not a keyword"
        );
        assert_eq!(
            context("select $tag$ ; from $tag$ , "),
            CompletionContext::Expression
        );
        assert_eq!(
            context("select \"from\" , "),
            CompletionContext::Expression,
            "quoted identifier is not a clause keyword"
        );
        assert_eq!(
            context("/* from */ select * from "),
            CompletionContext::Relation,
            "the clause after a block comment still counts"
        );
    }

    #[test]
    fn cursor_context_reports_the_word_and_its_range() {
        let cursor = cursor_context("select * from us", 10);
        assert_eq!(cursor.prefix, "us");
        assert_eq!(cursor.word, 24..26);
        assert_eq!(cursor.context, CompletionContext::Relation);

        let cursor = cursor_context("select u.em", 0);
        assert_eq!(
            cursor.context,
            CompletionContext::Qualifier(vec!["u".into()])
        );
        assert_eq!(cursor.prefix, "em");
        assert_eq!(cursor.word, 9..11);
    }

    #[test]
    fn unterminated_quoted_identifier_includes_the_quote_in_the_replacement() {
        let sql = "select * from \"us";
        let cursor = cursor_context(sql, 0);
        assert_eq!(cursor.context, CompletionContext::Relation);
        assert_eq!(cursor.prefix, "us");
        assert_eq!(cursor.word, 14..17);
    }

    // --- end-to-end completion against a synthetic catalog ---

    fn engine() -> SemanticEngine {
        SemanticEngine::new(catalog())
    }

    fn complete(engine: &SemanticEngine, sql: &str, offset: usize) -> Vec<CompletionItem> {
        let mut buffer = Buffer::new(SqlFileId::new(), sql);
        let file = buffer.file_id();
        engine.complete(CompletionRequest {
            file,
            buffer: &mut buffer,
            offset,
            explicit: true,
        })
    }

    fn labels(items: &[CompletionItem]) -> Vec<&str> {
        items.iter().map(|item| item.label.as_str()).collect()
    }

    #[test]
    fn statement_start_offers_statement_keywords() {
        let items = complete(&engine(), "sel", 3);
        assert_eq!(items.first().map(|i| i.label.as_str()), Some("SELECT"));
        assert_eq!(items[0].kind, CompletionKind::Keyword);

        // ctrl-space at an empty statement lists the whole set.
        let items = complete(&engine(), "", 0);
        assert!(labels(&items).contains(&"INSERT"));
        assert!(labels(&items).contains(&"WITH"));
    }

    #[test]
    fn relation_position_lists_ctes_before_tables() {
        let sql = "with c as (select 1 as x) select * from ";
        let items = complete(&engine(), sql, sql.len());
        let cte = items
            .iter()
            .position(|i| i.label == "c" && i.kind == CompletionKind::Cte)
            .expect("CTE candidate");
        let users = items
            .iter()
            .position(|i| i.label == "users" && i.kind == CompletionKind::Table)
            .expect("table candidate");
        assert!(
            cte < users,
            "CTEs rank above catalog relations: {:?}",
            labels(&items)
        );
        assert!(
            items
                .iter()
                .any(|i| i.label == "public" && i.kind == CompletionKind::Schema)
        );
    }

    #[test]
    fn expression_position_offers_columns_before_relations_and_keywords() {
        let sql = "select * from users u where ";
        let items = complete(&engine(), sql, sql.len());
        assert_eq!(items[0].label, "id");
        assert_eq!(items[0].kind, CompletionKind::Column);
        let first_relation = items.iter().position(|i| i.kind == CompletionKind::Table);
        let first_keyword = items.iter().position(|i| i.kind == CompletionKind::Keyword);
        assert!(
            first_relation.is_some_and(|r| r > 0),
            "columns outrank relations: {:?}",
            labels(&items)
        );
        assert!(first_keyword.is_some_and(|k| k > 0));
    }

    #[test]
    fn qualifier_dot_offers_only_that_bindings_columns() {
        // `select u. from users u` errors the FROM clause out of the tree in
        // the pinned grammar (probe), so the qualifier sits in a WHERE where
        // the statement keeps its bindings.
        let sql = "select * from users u where u.";
        let items = complete(&engine(), sql, sql.len());
        let mut got = labels(&items);
        got.sort_unstable();
        assert_eq!(got, vec!["email", "id", "name"]);
        assert!(items.iter().all(|i| i.kind == CompletionKind::Column));
    }

    #[test]
    fn schema_dot_offers_relations_of_that_schema() {
        let sql = "select * from public.";
        let items = complete(&engine(), sql, sql.len());
        let got = labels(&items);
        assert!(got.contains(&"users"), "{got:?}");
        assert!(got.contains(&"orders"), "{got:?}");
        assert!(!got.contains(&"id"), "no columns after a schema dot");
        assert!(items.iter().all(|i| i.kind == CompletionKind::Table));
    }

    #[test]
    fn schema_qualified_relation_dot_offers_its_columns() {
        let sql = "select * from public.users.";
        let items = complete(&engine(), sql, sql.len());
        let mut got = labels(&items);
        got.sort_unstable();
        assert_eq!(got, vec!["email", "id", "name"]);
        assert!(items.iter().all(|i| i.kind == CompletionKind::Column));
    }

    #[test]
    fn inner_scope_alias_shadows_the_outer_one() {
        let sql = "select * from users u where exists (select 1 from orders u where u.";
        let items = complete(&engine(), sql, sql.len());
        let mut got = labels(&items);
        got.sort_unstable();
        assert_eq!(
            got,
            vec!["id", "total", "user_id"],
            "the inner `orders u` wins over the outer `users u`"
        );
    }

    #[test]
    fn ambiguous_columns_are_offered_qualified() {
        let sql = "select * from users u join orders o on o.user_id = u.id where ";
        let items = complete(&engine(), sql, sql.len());
        let got = labels(&items);
        assert!(got.contains(&"u.id"), "{got:?}");
        assert!(got.contains(&"o.id"), "{got:?}");
        assert!(!got.contains(&"id"), "ambiguous bare column is not offered");
        assert!(got.contains(&"email"), "unique columns stay bare");
    }

    #[test]
    fn functions_carry_their_signature() {
        let sql = "select total from ";
        let items = complete(&engine(), sql, "select total".len());
        let function = items
            .iter()
            .find(|i| i.label == "total_sales")
            .expect("function candidate");
        assert_eq!(function.kind, CompletionKind::Function);
        assert_eq!(function.detail, "(region text) → numeric");
    }

    #[test]
    fn alias_position_offers_nothing() {
        let sql = "select id as ";
        assert!(complete(&engine(), sql, sql.len()).is_empty());
    }

    #[test]
    fn auto_open_requires_a_word_or_a_dot() {
        let engine = engine();
        let mut buffer = Buffer::new(SqlFileId::new(), "select * from ");
        let file = buffer.file_id();
        let offset = buffer.len();
        assert!(
            engine
                .complete(CompletionRequest {
                    file,
                    buffer: &mut buffer,
                    offset,
                    explicit: false,
                })
                .is_empty(),
            "whitespace alone does not open the popup"
        );
        assert!(
            !engine
                .complete(CompletionRequest {
                    file,
                    buffer: &mut buffer,
                    offset,
                    explicit: true,
                })
                .is_empty(),
            "ctrl-space forces it"
        );
    }

    #[test]
    fn replace_range_covers_the_typed_word() {
        let sql = "select * from us";
        let items = complete(&engine(), sql, sql.len());
        assert!(!items.is_empty());
        assert!(items.iter().all(|i| i.replace == (14..16)), "replace range");
    }

    #[test]
    fn completion_reflects_an_edit_through_the_revision_cache() {
        let engine = engine();
        let mut buffer = Buffer::new(SqlFileId::new(), "select * from us");
        let file = buffer.file_id();
        let offset = buffer.len();
        let first = engine.complete(CompletionRequest {
            file,
            buffer: &mut buffer,
            offset,
            explicit: true,
        });
        assert!(labels(&first).contains(&"users"));

        buffer.edit(&[(14..16, "or")]).expect("edit");
        let offset = buffer.len();
        let second = engine.complete(CompletionRequest {
            file,
            buffer: &mut buffer,
            offset,
            explicit: true,
        });
        assert!(labels(&second).contains(&"orders"), "{:?}", labels(&second));
        assert!(!labels(&second).contains(&"users"));
    }

    #[test]
    fn empty_catalog_offers_keywords_only() {
        let engine = SemanticEngine::new(empty_catalog());
        let items = complete(&engine, "select * from us", "select * from us".len());
        assert!(!items.is_empty());
        assert!(items.iter().all(|i| i.kind == CompletionKind::Keyword));
    }

    #[test]
    fn results_are_capped_and_deterministic() {
        let engine = engine();
        let sql = "select ";
        let first = complete(&engine, sql, sql.len());
        let second = complete(&engine, sql, sql.len());
        assert_eq!(labels(&first), labels(&second));
        assert!(first.len() <= COMPLETION_LIMIT);
        let mut sorted = first.clone();
        sorted.sort_by(|a, b| b.score.cmp(&a.score).then_with(|| a.label.cmp(&b.label)));
        assert_eq!(labels(&first), labels(&sorted));
    }

    #[test]
    fn the_cap_keeps_the_best_candidates() {
        let candidates = (0..300)
            .map(|i| candidate(&format!("c{i:03}"), 450))
            .collect();
        let ranked = rank(candidates, "", 0..0);
        assert_eq!(ranked.len(), COMPLETION_LIMIT);
        assert!(ranked.iter().any(|i| i.label == "c000"));
        assert!(!ranked.iter().any(|i| i.label == "c299"));
    }

    #[test]
    fn an_error_only_statement_does_not_borrow_the_previous_scopes() {
        let sql = "select * from users u; select * frm ";
        let items = complete(&engine(), sql, sql.len());
        let got = labels(&items);
        assert!(!got.contains(&"email"), "no columns from `users`: {got:?}");
        assert!(!got.contains(&"u.id"), "no qualified columns: {got:?}");
    }

    #[test]
    fn a_duplicate_output_name_in_one_binding_is_not_cross_binding_ambiguity() {
        let sql = "select * from (select users.id as id, users.email as id from users) c where ";
        let items = complete(&engine(), sql, sql.len());
        assert!(labels(&items).contains(&"id"), "{:?}", labels(&items));
    }

    #[test]
    fn ranking_prefers_exact_prefix_over_folded_over_subsequence() {
        let mut candidates = vec![
            candidate("Users", 100),
            candidate("user_sessions", 100),
            candidate("u_s_e_r_s", 100),
        ];
        let ranked = rank(std::mem::take(&mut candidates), "Users", 0..0);
        assert_eq!(
            ranked.first().map(|i| i.label.as_str()),
            Some("Users"),
            "exact prefix wins"
        );

        let ranked = rank(
            vec![
                candidate("Users", 100),
                candidate("user_sessions", 100),
                candidate("u_s_e_r_s", 100),
            ],
            "users",
            0..0,
        );
        assert_eq!(
            ranked.first().map(|i| i.label.as_str()),
            Some("Users"),
            "folded prefix beats a subsequence"
        );
    }

    #[test]
    fn dedup_keeps_one_item_per_label_and_kind() {
        let ranked = rank(
            vec![candidate("users", 450), candidate("users", 100)],
            "",
            0..0,
        );
        assert_eq!(ranked.len(), 1);
        assert_eq!(ranked[0].score, 450_000 - 5);
    }

    fn candidate(label: &str, weight: i32) -> Candidate {
        Candidate {
            label: label.to_string(),
            insert: label.to_string(),
            kind: CompletionKind::Table,
            detail: String::new(),
            weight,
        }
    }

    // Shared fixtures live next to the catalog tests.
    mod tempr_intel_catalog_test_support {
        use super::*;
        use std::sync::Arc;

        pub fn catalog() -> Arc<CatalogCache> {
            let users = SchemaObjectId::new();
            let orders = SchemaObjectId::new();
            let connection = ConnectionId::new();
            let mut objects = vec![
                SchemaObject::Table {
                    id: users,
                    schema: "public".into(),
                    name: "users".into(),
                    estimated_rows: Some(10),
                },
                SchemaObject::Table {
                    id: orders,
                    schema: "public".into(),
                    name: "orders".into(),
                    estimated_rows: Some(20),
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
            for (parent, name, ty, ordinal) in [
                (users, "id", "int8", 0),
                (users, "email", "text", 1),
                (users, "name", "text", 2),
                (orders, "id", "int8", 0),
                (orders, "user_id", "int8", 1),
                (orders, "total", "numeric", 2),
            ] {
                objects.push(SchemaObject::Column {
                    id: SchemaObjectId::new(),
                    parent_id: parent,
                    name: name.into(),
                    data_type: ty.into(),
                    nullable: true,
                    ordinal,
                    default: None,
                });
            }
            Arc::new(CatalogCache::from_snapshot(&SchemaSnapshot {
                id: SchemaSnapshotId::new(),
                connection_id: connection,
                version: 1,
                fetched_at: chrono::Utc::now(),
                objects,
                keywords: vec!["select".into(), "from".into(), "where".into()],
                fingerprints: vec![],
            }))
        }

        pub fn empty_catalog() -> Arc<CatalogCache> {
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
    }
}
