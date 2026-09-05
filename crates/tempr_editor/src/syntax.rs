//! `SyntaxTree` — incremental tree-sitter parse of a buffer (docs/10-editor.md).
//!
//! Grammar: `tree-sitter-sequel` (PostgreSQL-flavoured SQL, D22). The tree is
//! owned by `Buffer`, edited with every change (`tree.edit`) and re-parsed
//! incrementally from the rope's chunks — no full-text copy on the edit path.

use std::ops::Range;

use ropey::Rope;
use tree_sitter::{
    InputEdit, Language, Node, Parser, Point as TsPoint, Query, QueryCursor, StreamingIterator,
    Tree,
};

/// What a top-level range is. Callers that execute SQL should refuse
/// `Error` ranges (tree-sitter recovery fragments) rather than send them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatementKind {
    /// A single `statement` (the common case).
    Statement,
    /// `BEGIN … COMMIT/ROLLBACK` — one range spanning the whole transaction.
    Transaction,
    /// `BEGIN … END` block — one range spanning the whole block.
    Block,
    /// Text the parser could not fit into a statement.
    Error,
}

/// The byte range of one top-level statement (exclusive end), including its
/// terminating `;` when present.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StatementRange {
    pub start: usize,
    pub end: usize,
    pub kind: StatementKind,
}

impl StatementRange {
    /// `start <= offset < end`; see `SyntaxTree::statement_at` for the
    /// end-of-document special case.
    pub fn contains(&self, offset: usize) -> bool {
        self.start <= offset && offset < self.end
    }

    pub fn is_error(&self) -> bool {
        self.kind == StatementKind::Error
    }
}

fn kind_of(node: &Node) -> Option<StatementKind> {
    Some(match node.kind() {
        "statement" => StatementKind::Statement,
        "transaction" => StatementKind::Transaction,
        "block" => StatementKind::Block,
        _ if node.is_error() => StatementKind::Error,
        _ => return None,
    })
}

/// A highlight capture: byte range + capture name from `highlights.scm`
/// (e.g. `keyword`, `string`, `comment`, `function.call`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Highlight {
    pub range: Range<usize>,
    pub capture: String,
}

pub struct SyntaxTree {
    parser: Parser,
    tree: Tree,
    language: Language,
}

impl std::fmt::Debug for SyntaxTree {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SyntaxTree")
            .field("root", &self.tree.root_node().kind())
            .field("has_error", &self.tree.root_node().has_error())
            .finish()
    }
}

/// Tempr's highlight query source (see `queries/highlights.scm`).
pub const HIGHLIGHTS_QUERY: &str = include_str!("../queries/highlights.scm");

/// The SQL language handle (shared, cheap to clone).
pub fn language() -> Language {
    Language::from(tree_sitter_sequel::LANGUAGE)
}

impl SyntaxTree {
    /// Parse `text` from scratch.
    pub fn parse(text: &Rope) -> Self {
        let language = language();
        let mut parser = Parser::new();
        // A grammar/runtime ABI mismatch is a build configuration error
        // (both are pinned in Cargo.toml), not a runtime condition.
        #[allow(clippy::expect_used)]
        parser
            .set_language(&language)
            .expect("tree-sitter-sequel grammar ABI matches the tree-sitter runtime");
        let tree = parse_rope(&mut parser, text, None);
        Self {
            parser,
            tree,
            language,
        }
    }

    /// Tell the tree about a text change that has already been applied to
    /// the rope; call `reparse` once all edits of a transaction are in.
    pub fn edit(&mut self, edit: &InputEdit) {
        self.tree.edit(edit);
    }

    /// Incrementally re-parse against the (already edited) `text`.
    pub fn reparse(&mut self, text: &Rope) {
        self.tree = parse_rope(&mut self.parser, text, Some(&self.tree));
    }

    pub fn root_node(&self) -> Node<'_> {
        self.tree.root_node()
    }

    pub fn language(&self) -> &Language {
        &self.language
    }

    /// True when the tree contains syntax errors (ERROR or MISSING nodes).
    pub fn has_error(&self) -> bool {
        self.tree.root_node().has_error()
    }

    /// Top-level ranges in document order: every `statement`, `transaction`,
    /// `block` or `ERROR` child of the root, with a directly following `;`
    /// folded in. Comments and stray `;` are not statements.
    pub fn statement_ranges(&self) -> Vec<StatementRange> {
        let root = self.tree.root_node();
        let mut cursor = root.walk();
        let mut ranges: Vec<StatementRange> = Vec::new();
        for child in root.children(&mut cursor) {
            if child.kind() == ";" {
                if let Some(last) = ranges.last_mut()
                    && last.end == child.start_byte()
                {
                    last.end = child.end_byte();
                }
                continue;
            }
            if let Some(kind) = kind_of(&child) {
                ranges.push(StatementRange {
                    start: child.start_byte(),
                    end: child.end_byte(),
                    kind,
                });
            }
        }
        ranges
    }

    /// The top-level range containing `offset` (`start <= offset < end`,
    /// terminating `;` included). At the very end of the document the last
    /// range is returned if the offset touches it. `None` on comments,
    /// whitespace between statements, or stray `;`. O(log n): no allocation.
    pub fn statement_at(&self, offset: usize) -> Option<StatementRange> {
        let root = self.tree.root_node();
        let mut node = root.first_child_for_byte(offset).or_else(|| {
            // Past the last child: the last top-level node, if any.
            root.child(root.child_count().checked_sub(1)?)
        })?;
        // A `;` belongs to the range it terminates.
        if node.kind() == ";" {
            node = node.prev_sibling()?;
        }
        let kind = kind_of(&node)?;
        let mut end = node.end_byte();
        if let Some(next) = node.next_sibling()
            && next.kind() == ";"
            && next.start_byte() == end
        {
            end = next.end_byte();
        }
        let range = StatementRange {
            start: node.start_byte(),
            end,
            kind,
        };
        let at_document_end = offset == end && end == root.end_byte();
        (range.contains(offset) || at_document_end).then_some(range)
    }

    /// Run `query` over `byte_range` and return one capture per node range in
    /// document order. When several patterns capture the same range, the
    /// later pattern in the query file wins (so specific patterns such as
    /// numeric literals override generic ones). `text` feeds `#match?`.
    pub fn highlights(
        &self,
        query: &Query,
        text: &Rope,
        byte_range: Range<usize>,
    ) -> Vec<Highlight> {
        let names = query.capture_names();
        let mut cursor = QueryCursor::new();
        cursor.set_byte_range(byte_range);
        let provider = RopeText(text);
        let mut raw: Vec<(Range<usize>, usize, &str)> = Vec::new();
        let mut captures = cursor.captures(query, self.tree.root_node(), provider);
        while let Some((m, ix)) = captures.next() {
            let cap = m.captures[*ix];
            raw.push((
                cap.node.byte_range(),
                m.pattern_index,
                names[cap.index as usize],
            ));
        }
        raw.sort_by_key(|(r, pattern, _)| (r.start, r.end, *pattern));
        let mut out: Vec<Highlight> = Vec::with_capacity(raw.len());
        for (range, _, name) in raw {
            match out.last_mut() {
                Some(last) if last.range == range => last.capture = name.to_string(),
                _ => out.push(Highlight {
                    range,
                    capture: name.to_string(),
                }),
            }
        }
        out
    }

    /// Tempr's `highlights.scm` (`queries/highlights.scm`, derived from the
    /// grammar's), compiled once per process.
    pub fn highlight_query() -> &'static Query {
        static QUERY: std::sync::OnceLock<Query> = std::sync::OnceLock::new();
        QUERY.get_or_init(|| {
            // The query is part of this crate; a compile failure is a bug
            // caught by `highlights_yield_keyword_string_and_number_captures`.
            #[allow(clippy::expect_used)]
            Query::new(&language(), HIGHLIGHTS_QUERY)
                .expect("queries/highlights.scm compiles against tree-sitter-sequel")
        })
    }
}

fn parse_rope(parser: &mut Parser, text: &Rope, old: Option<&Tree>) -> Tree {
    // `None` only with a timeout / cancellation flag, neither of which we set.
    #[allow(clippy::expect_used)]
    parser
        .parse_with_options(
            &mut |byte, _pos: TsPoint| -> &[u8] {
                if byte >= text.len_bytes() {
                    return &[];
                }
                let (chunk, chunk_start, _, _) = text.chunk_at_byte(byte);
                &chunk.as_bytes()[byte - chunk_start..]
            },
            old,
            None,
        )
        .expect("parse is infallible without a timeout or cancellation flag")
}

/// `TextProvider` over the rope for query predicates.
struct RopeText<'a>(&'a Rope);

impl<'a> tree_sitter::TextProvider<&'a [u8]> for RopeText<'a> {
    type I = std::iter::Map<ropey::iter::Chunks<'a>, fn(&'a str) -> &'a [u8]>;

    fn text(&mut self, node: Node) -> Self::I {
        self.0
            .byte_slice(node.byte_range())
            .chunks()
            .map(str::as_bytes as fn(&'a str) -> &'a [u8])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rope(s: &str) -> Rope {
        Rope::from_str(s)
    }

    const SAMPLE: &str = "select 1;\n-- c\nselect a from t where b = 'x';\ncreate function f() returns int as $$ select 1; $$ language sql;\nupdate t set a = 1";

    #[test]
    fn parses_program_with_statements_and_no_errors() {
        let t = SyntaxTree::parse(&rope(SAMPLE));
        assert_eq!(t.root_node().kind(), "program");
        assert!(!t.has_error());
        let sexp = t.root_node().to_sexp();
        assert!(sexp.contains("(create_function"), "{sexp}");
        assert!(sexp.contains("(dollar_quote)"), "{sexp}");
    }

    #[test]
    fn statement_ranges_fold_semicolons_skip_comments_and_keep_dollar_bodies_whole() {
        let t = SyntaxTree::parse(&rope(SAMPLE));
        let ranges = t.statement_ranges();
        let texts: Vec<&str> = ranges.iter().map(|r| &SAMPLE[r.start..r.end]).collect();
        assert_eq!(
            texts,
            vec![
                "select 1;",
                "select a from t where b = 'x';",
                "create function f() returns int as $$ select 1; $$ language sql;",
                "update t set a = 1",
            ]
        );
    }

    #[test]
    fn statement_at_offsets() {
        let t = SyntaxTree::parse(&rope(SAMPLE));
        let ranges = t.statement_ranges();
        assert!(ranges.iter().all(|r| r.kind == StatementKind::Statement));
        assert_eq!(t.statement_at(0), Some(ranges[0]));
        assert_eq!(t.statement_at(8), Some(ranges[0]), "on the semicolon");
        assert_eq!(
            t.statement_at(9),
            None,
            "past the semicolon, on the newline"
        );
        assert_eq!(t.statement_at(12), None, "inside the comment line");
        let inner = SAMPLE.find("$$ select").unwrap() + 4;
        assert_eq!(t.statement_at(inner), Some(ranges[2]), "inside a $$ body");
        assert_eq!(t.statement_at(SAMPLE.len()), Some(ranges[3]), "at EOF");
        assert_eq!(
            SyntaxTree::parse(&rope("-- only a comment\n")).statement_at(3),
            None
        );
        // Adjacent statements: the boundary offset belongs to the second one.
        let two = SyntaxTree::parse(&rope("select 1;select 2;"));
        assert_eq!(two.statement_at(9).unwrap().start, 9);
        assert_eq!(two.statement_at(8).unwrap().start, 0);
        // statement_at agrees with statement_ranges everywhere.
        for off in 0..=SAMPLE.len() {
            let expected = ranges
                .iter()
                .copied()
                .find(|r| r.contains(off) || (off == SAMPLE.len() && off == r.end));
            assert_eq!(t.statement_at(off), expected, "offset {off}");
        }
    }

    #[test]
    fn error_fragments_and_stray_semicolons_are_not_statements() {
        // The grammar reports a leading `;;` as an ERROR node: it must never be
        // promoted to a runnable statement.
        let src = ";; select 1;";
        let t = SyntaxTree::parse(&rope(src));
        let good: Vec<&str> = t
            .statement_ranges()
            .iter()
            .filter(|r| !r.is_error())
            .map(|r| &src[r.start..r.end])
            .collect();
        assert_eq!(good, vec!["select 1;"]);
        assert!(
            t.statement_at(0).is_none_or(|r| r.is_error()),
            "stray ; is not a runnable statement: {:?}",
            t.statement_at(0)
        );

        let sql = "select 1;\nselect 2 from;\nselect 3;";
        let t = SyntaxTree::parse(&rope(sql));
        assert!(t.has_error());
        let ranges = t.statement_ranges();
        let good: Vec<&str> = ranges
            .iter()
            .filter(|r| !r.is_error())
            .map(|r| &sql[r.start..r.end])
            .collect();
        assert!(
            good.contains(&"select 1;") && good.contains(&"select 3;"),
            "{good:?}"
        );
        assert!(
            ranges.iter().any(|r| r.is_error()),
            "recovery fragments are flagged, not silently promoted: {ranges:?}"
        );
    }

    #[test]
    fn blocks_and_transactions_are_single_ranges_with_their_kind() {
        let sql = "begin; select 1; select 2; end;";
        let t = SyntaxTree::parse(&rope(sql));
        let ranges = t.statement_ranges();
        assert_eq!(ranges.len(), 1, "{ranges:?}");
        assert!(matches!(
            ranges[0].kind,
            StatementKind::Block | StatementKind::Transaction
        ));
        assert_eq!(&sql[ranges[0].start..ranges[0].end], sql);
        assert_eq!(t.statement_at(10).map(|r| r.kind), Some(ranges[0].kind));
    }

    #[test]
    fn statement_boundaries_ignore_semicolons_inside_strings_comments_and_dollar_bodies() {
        let sql = "select ';' as a; -- trailing; comment\n/* block ; comment */ select 'x;y', $tag$ a; b $tag$; select 3";
        let t = SyntaxTree::parse(&rope(sql));
        assert!(!t.has_error(), "{}", t.root_node().to_sexp());
        let texts: Vec<&str> = t
            .statement_ranges()
            .iter()
            .map(|r| &sql[r.start..r.end])
            .collect();
        assert_eq!(
            texts,
            vec![
                "select ';' as a;",
                "select 'x;y', $tag$ a; b $tag$;",
                "select 3"
            ]
        );
        // Offsets inside the comment and the block comment belong to no statement.
        assert_eq!(t.statement_at(sql.find("-- trailing").unwrap() + 3), None);
        assert_eq!(t.statement_at(sql.find("/* block").unwrap() + 3), None);
        // Inside the dollar-quoted body → the enclosing statement.
        let inside = sql.find("a; b").unwrap() + 1;
        assert_eq!(&sql[t.statement_at(inside).unwrap().start..][..6], "select");
    }

    #[test]
    fn syntax_errors_are_reported() {
        let t = SyntaxTree::parse(&rope("select (1 from t where;"));
        assert!(t.has_error(), "{}", t.root_node().to_sexp());
        let ok = SyntaxTree::parse(&rope("select 1"));
        assert!(!ok.has_error());
    }

    #[test]
    fn highlights_yield_keyword_string_and_number_captures() {
        let src = "select 42, 1.5, 'x' -- c\nfrom t;";
        let text = rope(src);
        let t = SyntaxTree::parse(&text);
        let hs = t.highlights(SyntaxTree::highlight_query(), &text, 0..text.len_bytes());
        let by_text: Vec<(&str, &str)> = hs
            .iter()
            .map(|h| (&src[h.range.clone()], h.capture.as_str()))
            .collect();
        assert!(by_text.contains(&("select", "keyword")), "{by_text:?}");
        assert!(by_text.contains(&("42", "number")), "{by_text:?}");
        assert!(by_text.contains(&("1.5", "float")), "{by_text:?}");
        assert!(by_text.contains(&("'x'", "string")), "{by_text:?}");
        assert!(by_text.contains(&("-- c", "comment")), "{by_text:?}");
        // One capture per range: no duplicates, no @spell.
        let mut ranges: Vec<&Range<usize>> = hs.iter().map(|h| &h.range).collect();
        ranges.dedup();
        assert_eq!(ranges.len(), hs.len());
        assert!(hs.iter().all(|h| h.capture != "spell"));
    }

    #[test]
    fn incremental_reparse_matches_full_parse() {
        let mut text = rope("select 1;\nselect 2;");
        let mut t = SyntaxTree::parse(&text);
        // Insert " + 40" after "select 2" (byte 18).
        let at = 18;
        text.insert(text.byte_to_char(at), " + 40");
        t.edit(&InputEdit {
            start_byte: at,
            old_end_byte: at,
            new_end_byte: at + 5,
            start_position: TsPoint::new(1, 8),
            old_end_position: TsPoint::new(1, 8),
            new_end_position: TsPoint::new(1, 13),
        });
        t.reparse(&text);
        let full = SyntaxTree::parse(&text);
        assert_eq!(t.root_node().to_sexp(), full.root_node().to_sexp());
        assert_eq!(t.statement_ranges(), full.statement_ranges());
        assert_eq!(
            &text.to_string()[t.statement_ranges()[1].start..],
            "select 2 + 40;"
        );
    }
}
