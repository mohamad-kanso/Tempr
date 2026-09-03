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

/// The byte range of one SQL statement (exclusive end), including its
/// terminating `;` when present.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StatementRange {
    pub start: usize,
    pub end: usize,
}

impl StatementRange {
    pub fn contains(&self, offset: usize) -> bool {
        self.start <= offset && offset <= self.end
    }
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

    /// Top-level statement ranges in document order. Each `statement`
    /// (or `transaction`) child of the root; a following `;` is folded in.
    pub fn statement_ranges(&self) -> Vec<StatementRange> {
        let root = self.tree.root_node();
        let mut cursor = root.walk();
        let mut ranges: Vec<StatementRange> = Vec::new();
        for child in root.children(&mut cursor) {
            match child.kind() {
                ";" => {
                    if let Some(last) = ranges.last_mut()
                        && last.end <= child.start_byte()
                    {
                        last.end = child.end_byte();
                    }
                }
                "comment" | "marginalia" => {}
                _ => ranges.push(StatementRange {
                    start: child.start_byte(),
                    end: child.end_byte(),
                }),
            }
        }
        ranges
    }

    /// The statement containing `offset` (inclusive of its end), if any.
    pub fn statement_at(&self, offset: usize) -> Option<StatementRange> {
        self.statement_ranges()
            .into_iter()
            .find(|r| r.contains(offset))
    }

    /// Run `query` over `byte_range` and return captures in document order.
    /// `text` is needed for predicates such as `#match?`.
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
        let mut out = Vec::new();
        let mut captures = cursor.captures(query, self.tree.root_node(), provider);
        while let Some((m, ix)) = captures.next() {
            let cap = m.captures[*ix];
            out.push(Highlight {
                range: cap.node.byte_range(),
                capture: names[cap.index as usize].to_string(),
            });
        }
        out.sort_by_key(|h| (h.range.start, h.range.end));
        out
    }

    /// The grammar's bundled `highlights.scm`, compiled once per process.
    pub fn highlight_query() -> &'static Query {
        static QUERY: std::sync::OnceLock<Query> = std::sync::OnceLock::new();
        QUERY.get_or_init(|| {
            // The query ships inside the grammar crate; a compile failure is a
            // dependency bug caught by `highlights_yield_keyword_and_string_captures`.
            #[allow(clippy::expect_used)]
            Query::new(&language(), tree_sitter_sequel::HIGHLIGHTS_QUERY)
                .expect("bundled highlights.scm compiles against its own grammar")
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
        assert_eq!(t.statement_at(0), Some(ranges[0]));
        assert_eq!(t.statement_at(9), Some(ranges[0]), "on the semicolon");
        assert_eq!(t.statement_at(12), None, "inside the comment line");
        let inner = SAMPLE.find("$$ select").unwrap() + 4;
        assert_eq!(t.statement_at(inner), Some(ranges[2]), "inside a $$ body");
        assert_eq!(t.statement_at(SAMPLE.len()), Some(ranges[3]), "at EOF");
        assert_eq!(
            SyntaxTree::parse(&rope("-- only a comment\n")).statement_at(3),
            None
        );
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
    fn highlights_yield_keyword_and_string_captures() {
        let text = rope("select a from t where b = 'x';");
        let t = SyntaxTree::parse(&text);
        let hs = t.highlights(SyntaxTree::highlight_query(), &text, 0..text.len_bytes());
        let names: std::collections::BTreeSet<&str> =
            hs.iter().map(|h| h.capture.as_str()).collect();
        assert!(names.contains("keyword"), "{names:?}");
        assert!(names.contains("string"), "{names:?}");
        assert!(
            hs.iter()
                .any(|h| h.range == (0..6) && h.capture == "keyword")
        );
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
