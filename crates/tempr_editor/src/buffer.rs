//! `Buffer` — text storage (rope) + edit history for one SQL file.
//!
//! All offsets in this API are **byte offsets** into the UTF-8 text, matching
//! tree-sitter, `StatementRange`, and the result of `str` slicing. `ropey`
//! itself is char-indexed; the conversions live here and nowhere else.
//!
//! Line semantics: a line ends at `\n`; a preceding `\r` belongs to the
//! terminator (CRLF). Other Unicode separators are ordinary characters
//! (ropey is built without `unicode_lines`/`cr_lines`).
//!
//! `edit` returns `Result` (an edit with an out-of-range, non-char-boundary,
//! or overlapping range is a caller bug we refuse rather than panic on) and
//! `Ok(None)` when the batch changes nothing. The buffer does not publish
//! `BufferChanged` — it is a pure model; the owning service publishes.

use std::ops::Range;

use ropey::Rope;
use tempr_domain::SqlFileId;
use thiserror::Error;

/// Identifier of one applied edit transaction; monotonically increasing per
/// buffer. `undo`/`redo` return the id of the transaction they reverted or
/// re-applied.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct EditId(pub u64);

/// A (line, column) position. `column` is a **byte** offset within the line
/// (0-based); line breaks are not part of a line's columns.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Point {
    pub line: usize,
    pub column: usize,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum EditError {
    #[error("edit range {start}..{end} exceeds buffer length {len}")]
    OutOfBounds {
        start: usize,
        end: usize,
        len: usize,
    },
    #[error("edit range {start}..{end} is inverted")]
    InvertedRange { start: usize, end: usize },
    #[error("byte offset {offset} is not on a UTF-8 character boundary")]
    NotCharBoundary { offset: usize },
    #[error("edit ranges overlap: {a_start}..{a_end} and {b_start}..{b_end}")]
    Overlapping {
        a_start: usize,
        a_end: usize,
        b_start: usize,
        b_end: usize,
    },
}

/// One replacement inside a transaction, remembered for undo/redo.
///
/// `start` is the position in the pre-transaction text. Changes are applied
/// highest-start-first, so while a change is being applied every change with
/// a lower start is still unapplied and `start` is exact; undoing in LIFO
/// order restores that same situation, so `start` is exact again.
#[derive(Debug, Clone)]
struct Change {
    start: usize,
    removed: String,
    inserted: String,
}

#[derive(Debug, Clone)]
struct Transaction {
    id: EditId,
    /// In application order (start descending; see `edit` for tie-breaks).
    changes: Vec<Change>,
}

#[derive(Debug, Default)]
struct EditHistory {
    undo: Vec<Transaction>,
    redo: Vec<Transaction>,
}

pub struct Buffer {
    rope: Rope,
    history: EditHistory,
    file_id: SqlFileId,
    next_edit: u64,
}

impl Buffer {
    pub fn new(file_id: SqlFileId, text: &str) -> Self {
        Self {
            rope: Rope::from_str(text),
            history: EditHistory::default(),
            file_id,
            next_edit: 1,
        }
    }

    pub fn file_id(&self) -> SqlFileId {
        self.file_id
    }

    /// Total byte length of the content.
    pub fn len(&self) -> usize {
        self.rope.len_bytes()
    }

    pub fn is_empty(&self) -> bool {
        self.rope.len_bytes() == 0
    }

    /// Number of lines (a trailing line break starts a new, empty line —
    /// ropey semantics; an empty buffer has one line).
    pub fn len_lines(&self) -> usize {
        self.rope.len_lines()
    }

    /// A cheap snapshot of the text (ropey ropes share structure on clone).
    pub fn text(&self) -> Rope {
        self.rope.clone()
    }

    /// The content of `range` (byte offsets) as an owned `String`.
    pub fn slice(&self, range: Range<usize>) -> Result<String, EditError> {
        let chars = self.char_range(&range)?;
        Ok(self.rope.slice(chars).to_string())
    }

    /// Line `line` without its terminating line break; `None` past the end.
    pub fn line(&self, line: usize) -> Option<String> {
        if line >= self.rope.len_lines() {
            return None;
        }
        let start = self.rope.line_to_byte(line);
        let end = start + self.line_content_len(line);
        Some(self.rope.byte_slice(start..end).to_string())
    }

    /// Apply a batch of replacements expressed against the **current** text.
    ///
    /// Ranges must lie within the buffer, start and end on char boundaries,
    /// and not overlap (touching is fine). Edits sharing a start are ordered
    /// so that a zero-width insert lands *before* a replacement at the same
    /// offset, and several inserts at one offset appear in caller order.
    ///
    /// All-or-nothing: on error the buffer is unchanged. Returns `Ok(None)`
    /// and leaves the history alone when the batch changes no text
    /// (empty batch, or every replacement equals what it replaces); a real
    /// change records one undo transaction and clears the redo stack.
    pub fn edit(&mut self, edits: &[(Range<usize>, &str)]) -> Result<Option<EditId>, EditError> {
        // Validate everything before touching the rope.
        for (range, _) in edits {
            self.char_range(range)?;
        }
        // Application order: start descending; for equal starts the longer
        // range first (so an insert at that offset ends up in front of the
        // replaced text), then later caller entries first (so same-offset
        // inserts read in caller order).
        let mut order: Vec<usize> = (0..edits.len()).collect();
        order.sort_by(|&a, &b| {
            let (ra, rb) = (&edits[a].0, &edits[b].0);
            rb.start
                .cmp(&ra.start)
                .then(rb.end.cmp(&ra.end))
                .then(b.cmp(&a))
        });
        for w in order.windows(2) {
            let (hi, lo) = (&edits[w[0]].0, &edits[w[1]].0);
            // Two non-empty ranges sharing a start overlap by definition.
            let same_start_both_nonempty = lo.start == hi.start && !lo.is_empty() && !hi.is_empty();
            if lo.end > hi.start || same_start_both_nonempty {
                return Err(EditError::Overlapping {
                    a_start: lo.start,
                    a_end: lo.end,
                    b_start: hi.start,
                    b_end: hi.end,
                });
            }
        }

        // Apply from the highest start down so lower offsets stay valid.
        let mut changes: Vec<Change> = Vec::with_capacity(edits.len());
        for &i in &order {
            let (range, text) = &edits[i];
            let removed = self.replace_bytes(range.clone(), text);
            if removed != *text {
                changes.push(Change {
                    start: range.start,
                    removed,
                    inserted: (*text).to_string(),
                });
            }
        }
        if changes.is_empty() {
            return Ok(None);
        }

        let id = EditId(self.next_edit);
        self.next_edit += 1;
        self.history.undo.push(Transaction { id, changes });
        self.history.redo.clear();
        Ok(Some(id))
    }

    /// Revert the most recent transaction. Returns its id.
    pub fn undo(&mut self) -> Option<EditId> {
        let tx = self.history.undo.pop()?;
        // LIFO: the last-applied change (lowest start) goes first; by the time
        // a change is undone, everything below it is already undone, so its
        // original `start` is exact.
        for change in tx.changes.iter().rev() {
            let range = change.start..change.start + change.inserted.len();
            self.replace_bytes(range, &change.removed);
        }
        let id = tx.id;
        self.history.redo.push(tx);
        Some(id)
    }

    /// Re-apply the most recently undone transaction. Returns its id.
    pub fn redo(&mut self) -> Option<EditId> {
        let tx = self.history.redo.pop()?;
        for change in &tx.changes {
            let range = change.start..change.start + change.removed.len();
            self.replace_bytes(range, &change.inserted);
        }
        let id = tx.id;
        self.history.undo.push(tx);
        Some(id)
    }

    pub fn can_undo(&self) -> bool {
        !self.history.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.history.redo.is_empty()
    }

    /// (line, byte column) for a byte offset; offsets past the end clamp to
    /// the end of the text.
    pub fn point_for_offset(&self, offset: usize) -> Point {
        let offset = offset.min(self.len());
        let line = self.rope.byte_to_line(offset);
        let line_start = self.rope.line_to_byte(line);
        Point {
            line,
            column: offset - line_start,
        }
    }

    /// Byte offset for a point; the line clamps to the last line, the
    /// column clamps to the end of that line's content (before its break),
    /// and the result is snapped down to a char boundary so it is always
    /// valid for `edit`/`slice`.
    pub fn offset_for_point(&self, point: Point) -> usize {
        let last_line = self.rope.len_lines().saturating_sub(1);
        let line = point.line.min(last_line);
        let line_start = self.rope.line_to_byte(line);
        let byte = line_start + point.column.min(self.line_content_len(line));
        self.rope.char_to_byte(self.rope.byte_to_char(byte))
    }

    // ── internals ───────────────────────────────────────────────────────

    /// Byte length of `line` excluding its terminator (`\n` or `\r\n`).
    /// O(log n): two line→byte lookups plus at most two byte reads.
    fn line_content_len(&self, line: usize) -> usize {
        let start = self.rope.line_to_byte(line);
        let end = if line + 1 < self.rope.len_lines() {
            self.rope.line_to_byte(line + 1)
        } else {
            self.rope.len_bytes()
        };
        let mut len = end - start;
        if len > 0 && self.rope.byte(end - 1) == b'\n' {
            len -= 1;
            if len > 0 && self.rope.byte(end - 2) == b'\r' {
                len -= 1;
            }
        }
        len
    }

    /// Validate a byte range and convert it to ropey char indices.
    fn char_range(&self, range: &Range<usize>) -> Result<Range<usize>, EditError> {
        let len = self.len();
        if range.start > range.end {
            return Err(EditError::InvertedRange {
                start: range.start,
                end: range.end,
            });
        }
        if range.end > len {
            return Err(EditError::OutOfBounds {
                start: range.start,
                end: range.end,
                len,
            });
        }
        Ok(self.char_index(range.start)?..self.char_index(range.end)?)
    }

    fn char_index(&self, byte: usize) -> Result<usize, EditError> {
        let ch = self.rope.byte_to_char(byte);
        if self.rope.char_to_byte(ch) != byte {
            return Err(EditError::NotCharBoundary { offset: byte });
        }
        Ok(ch)
    }

    /// Replace a validated byte range; returns the removed text.
    fn replace_bytes(&mut self, range: Range<usize>, text: &str) -> String {
        let start = self.rope.byte_to_char(range.start);
        let end = self.rope.byte_to_char(range.end);
        let removed = self.rope.slice(start..end).to_string();
        if start != end {
            self.rope.remove(start..end);
        }
        if !text.is_empty() {
            self.rope.insert(start, text);
        }
        removed
    }
}

impl std::fmt::Debug for Buffer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Buffer")
            .field("file_id", &self.file_id)
            .field("len", &self.len())
            .field("lines", &self.len_lines())
            .field("undo_depth", &self.history.undo.len())
            .field("redo_depth", &self.history.redo.len())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn buf(text: &str) -> Buffer {
        Buffer::new(SqlFileId::new(), text)
    }

    #[test]
    fn new_buffer_reports_len_and_lines() {
        let b = buf("select 1;\nselect 2;");
        assert_eq!(b.len(), 19);
        assert_eq!(b.len_lines(), 2);
        assert_eq!(b.line(0).as_deref(), Some("select 1;"));
        assert_eq!(b.line(1).as_deref(), Some("select 2;"));
        assert_eq!(b.line(2), None);
        assert!(buf("").is_empty());
        assert_eq!(buf("").len_lines(), 1);
    }

    #[test]
    fn insert_delete_replace() {
        let mut b = buf("select 1;");
        b.edit(&[(6..6, " *")]).unwrap().unwrap(); // insert
        assert_eq!(b.text().to_string(), "select * 1;");
        b.edit(&[(8..10, "")]).unwrap(); // delete " 1"
        assert_eq!(b.text().to_string(), "select *;");
        b.edit(&[(0..6, "SELECT")]).unwrap(); // replace
        assert_eq!(b.text().to_string(), "SELECT *;");
    }

    #[test]
    fn batch_edits_use_original_offsets_regardless_of_order() {
        let mut b = buf("aaa bbb ccc");
        // Given in ascending order; the buffer must apply them safely.
        b.edit(&[(0..3, "A"), (4..7, "BBBBB"), (8..11, "")])
            .unwrap();
        assert_eq!(b.text().to_string(), "A BBBBB ");
        let mut b2 = buf("aaa bbb ccc");
        b2.edit(&[(8..11, ""), (4..7, "BBBBB"), (0..3, "A")])
            .unwrap();
        assert_eq!(b2.text().to_string(), "A BBBBB ");
    }

    #[test]
    fn undo_redo_single_and_batch_transactions() {
        let mut b = buf("aaa bbb ccc");
        let e1 = b
            .edit(&[(0..3, "A"), (4..7, "BBBBB"), (8..11, "")])
            .unwrap()
            .unwrap();
        let e2 = b.edit(&[(1..1, "-")]).unwrap().unwrap();
        assert_eq!(b.text().to_string(), "A- BBBBB ");

        assert_eq!(b.undo(), Some(e2));
        assert_eq!(b.text().to_string(), "A BBBBB ");
        assert_eq!(b.undo(), Some(e1));
        assert_eq!(b.text().to_string(), "aaa bbb ccc");
        assert_eq!(b.undo(), None);

        assert_eq!(b.redo(), Some(e1));
        assert_eq!(b.text().to_string(), "A BBBBB ");
        assert_eq!(b.redo(), Some(e2));
        assert_eq!(b.text().to_string(), "A- BBBBB ");
        assert_eq!(b.redo(), None);
    }

    #[test]
    fn new_edit_clears_redo() {
        let mut b = buf("x");
        b.edit(&[(1..1, "y")]).unwrap();
        b.undo();
        assert!(b.can_redo());
        b.edit(&[(1..1, "z")]).unwrap();
        assert!(!b.can_redo());
        assert_eq!(b.text().to_string(), "xz");
    }

    #[test]
    fn edit_ids_increase_and_survive_undo_redo() {
        let mut b = buf("");
        let a = b.edit(&[(0..0, "a")]).unwrap().unwrap();
        let c = b.edit(&[(1..1, "c")]).unwrap().unwrap();
        assert!(c > a);
        assert_eq!(b.undo(), Some(c));
        assert_eq!(b.redo(), Some(c));
        let d = b.edit(&[(2..2, "d")]).unwrap().unwrap();
        assert!(d > c);
    }

    #[test]
    fn multibyte_text_edits_and_boundaries() {
        let mut b = buf("héllo wörld"); // é and ö are 2 bytes each
        assert_eq!(b.len(), 13);
        b.edit(&[(1..3, "e")]).unwrap(); // replace é
        assert_eq!(b.text().to_string(), "hello wörld");
        assert_eq!(
            b.edit(&[(8..8, "x")]), // inside ö (bytes 7..9)
            Err(EditError::NotCharBoundary { offset: 8 })
        );
        assert_eq!(
            b.text().to_string(),
            "hello wörld",
            "failed edit leaves text intact"
        );
        assert_eq!(b.slice(6..9).unwrap(), "wö");
    }

    #[test]
    fn invalid_ranges_are_rejected_atomically() {
        let mut b = buf("abc");
        assert_eq!(
            b.edit(&[(0..1, "X"), (2..9, "")]),
            Err(EditError::OutOfBounds {
                start: 2,
                end: 9,
                len: 3
            })
        );
        assert_eq!(b.text().to_string(), "abc");
        assert!(matches!(
            b.edit(&[(0..2, "X"), (1..3, "Y")]),
            Err(EditError::Overlapping { .. })
        ));
        assert!(matches!(
            b.edit(&[(std::ops::Range { start: 2, end: 1 }, "")]),
            Err(EditError::InvertedRange { .. })
        ));
        // Touching ranges are fine.
        b.edit(&[(0..1, "X"), (1..3, "Y")]).unwrap();
        assert_eq!(b.text().to_string(), "XY");
        assert!(!b.can_redo());
    }

    #[test]
    fn point_offset_roundtrip_with_crlf_and_clamping() {
        // bytes: a0 b1 \r2 \n3 | c4 d5 é6-7 \n8 | f9   (len 10)
        let b = buf("ab\r\ncdé\nf");
        assert_eq!(b.len(), 10);
        assert_eq!(b.len_lines(), 3);
        assert_eq!(b.point_for_offset(0), Point { line: 0, column: 0 });
        assert_eq!(b.point_for_offset(4), Point { line: 1, column: 0 });
        // before 'é'
        assert_eq!(b.point_for_offset(6), Point { line: 1, column: 2 });
        // after 'é', before the line break
        assert_eq!(b.point_for_offset(8), Point { line: 1, column: 4 });
        assert_eq!(b.offset_for_point(Point { line: 1, column: 4 }), 8);
        // Column past the line content clamps before the break.
        assert_eq!(
            b.offset_for_point(Point {
                line: 0,
                column: 99
            }),
            2
        );
        assert_eq!(
            b.offset_for_point(Point {
                line: 1,
                column: 99
            }),
            8
        );
        // Line past the end clamps to the last line.
        assert_eq!(
            b.offset_for_point(Point {
                line: 42,
                column: 0
            }),
            9
        );
        // Offset past the end clamps to the end.
        assert_eq!(b.point_for_offset(999), Point { line: 2, column: 1 });
        for off in [0, 1, 2, 4, 5, 6, 8, 9, 10] {
            assert_eq!(
                b.offset_for_point(b.point_for_offset(off)),
                off,
                "offset {off}"
            );
        }
    }

    /// Phase 2 acceptance: insert/delete at a mid-document position in a
    /// 10 MB file completes in < 1 ms. Timing-sensitive → ignored by default;
    /// run with `cargo test -p tempr_editor --release -- --ignored`.
    #[test]
    #[ignore = "timing-sensitive; run in release on a quiet machine"]
    fn perf_10mb_insert_delete_under_1ms() {
        use std::time::{Duration, Instant};
        let line = "select id, name, created_at from accounts where id = 42;\n";
        let target = 10 * 1024 * 1024;
        let text = line.repeat(target / line.len() + 1);
        let mut b = buf(&text);
        assert!(b.len() >= target);

        let mid = b.len() / 2;
        // Land on a line start so we are on a char boundary.
        let mid = b.offset_for_point(Point {
            line: b.point_for_offset(mid).line,
            column: 0,
        });
        let mut worst = Duration::ZERO;
        let mut total = Duration::ZERO;
        let iterations: u32 = 200;
        for i in 0..iterations {
            let at = mid + (i as usize % 7) * line.len();
            let t = Instant::now();
            b.edit(&[(at..at, "-- inserted\n")]).unwrap();
            b.edit(&[(at..at + 12, "")]).unwrap();
            let d = t.elapsed();
            total += d;
            worst = worst.max(d);
        }
        let avg = total / iterations;
        eprintln!("10 MB insert+delete: avg {avg:?}, worst {worst:?}");
        assert!(
            avg < Duration::from_millis(1),
            "average insert+delete {avg:?} exceeds 1 ms"
        );
    }

    #[test]
    fn same_start_insert_and_replace_undo_exactly() {
        // Zero-width insert at the start of a replaced range: the insert
        // lands before the replacement, and undo restores the original.
        for edits in [
            vec![(2..5, "Z"), (2..2, "x")],
            vec![(2..2, "x"), (2..5, "Z")],
        ] {
            let mut b = buf("abcdefg");
            b.edit(&edits).unwrap().unwrap();
            assert_eq!(b.text().to_string(), "abxZfg");
            b.undo();
            assert_eq!(b.text().to_string(), "abcdefg");
            b.redo();
            assert_eq!(b.text().to_string(), "abxZfg");
        }
        let mut b = buf("01234X");
        b.edit(&[(5..6, "c"), (5..5, "ab")]).unwrap().unwrap();
        assert_eq!(b.text().to_string(), "01234abc");
        b.undo();
        assert_eq!(b.text().to_string(), "01234X");
    }

    #[test]
    fn same_offset_inserts_keep_caller_order() {
        let mut b = buf("abcd");
        b.edit(&[(2..2, "x"), (2..2, "y")]).unwrap().unwrap();
        assert_eq!(b.text().to_string(), "abxycd");
        b.undo();
        assert_eq!(b.text().to_string(), "abcd");
        // Two non-empty ranges at one start overlap.
        assert!(matches!(
            b.edit(&[(1..2, "p"), (1..3, "q")]),
            Err(EditError::Overlapping { .. })
        ));
    }

    #[test]
    fn no_op_batches_do_not_touch_history() {
        let mut b = buf("abcd");
        b.edit(&[(2..2, "c")]).unwrap().unwrap();
        b.undo();
        assert!(b.can_redo());
        assert_eq!(b.edit(&[]).unwrap(), None);
        assert_eq!(b.edit(&[(1..1, "")]).unwrap(), None);
        assert_eq!(
            b.edit(&[(1..3, "bc")]).unwrap(),
            None,
            "identical replacement"
        );
        assert!(b.can_redo(), "redo history survives no-op edits");
        assert_eq!(b.text().to_string(), "abcd");
        // A batch with one real change and one no-op records just the change.
        let id = b.edit(&[(0..1, "a"), (4..4, "!")]).unwrap().unwrap();
        assert_eq!(b.text().to_string(), "abcd!");
        assert_eq!(b.undo(), Some(id));
        assert_eq!(b.text().to_string(), "abcd");
    }

    #[test]
    fn offset_for_point_snaps_to_char_boundary() {
        let b = buf("ab\ncdé"); // bytes: a0 b1 \n2 | c3 d4 é5-6
        let off = b.offset_for_point(Point { line: 1, column: 3 });
        assert_eq!(off, 5, "column inside é snaps back to its start");
        let mut b2 = buf("ab\ncdé");
        b2.edit(&[(off..off, "x")]).unwrap().unwrap();
        assert_eq!(b2.text().to_string(), "ab\ncdxé");
    }

    #[test]
    fn only_lf_and_crlf_are_line_breaks() {
        let b = buf("ab\u{2028}cd\u{0c}ef\nx");
        assert_eq!(b.len_lines(), 2, "U+2028 and form feed are not breaks");
        assert_eq!(b.line(0).as_deref(), Some("ab\u{2028}cd\u{0c}ef"));
        assert_eq!(
            b.offset_for_point(Point {
                line: 0,
                column: 99
            }),
            "ab\u{2028}cd\u{0c}ef".len()
        );
    }

    #[test]
    fn large_batch_edit_is_fast_enough() {
        // 20k single-char replacements in one transaction (a "replace all").
        let text = "a".repeat(200_000);
        let mut b = buf(&text);
        let edits: Vec<(Range<usize>, &str)> =
            (0..20_000).map(|i| (i * 10..i * 10 + 1, "b")).collect();
        let t = std::time::Instant::now();
        b.edit(&edits).unwrap().unwrap();
        let elapsed = t.elapsed();
        assert_eq!(b.slice(0..11).unwrap(), "baaaaaaaaab");
        // Generous bound for debug builds; the point is no quadratic blow-up.
        assert!(elapsed.as_millis() < 2_000, "batch edit took {elapsed:?}");
        b.undo();
        assert_eq!(b.text().to_string(), text);
    }
}
