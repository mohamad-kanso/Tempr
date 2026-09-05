//! Cursor-based editing operations: each op takes the current selections,
//! applies one atomic `Buffer::edit` batch (docs/10-editor.md → Multi-Range
//! Editing), and returns the selections to use afterwards. Undo/redo restore
//! the recorded selections.

use crate::buffer::{Buffer, EditError, EditId, Point};
use crate::selection::{Selection, normalize};

/// Result of an editing operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditOutcome {
    /// `None` when the operation changed no text.
    pub edit: Option<EditId>,
    /// Selections after the operation (normalized).
    pub selections: Vec<Selection>,
}

/// Direction for line moves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineDirection {
    Up,
    Down,
}

impl Buffer {
    /// Replace every selection with `text` (typing, paste). Cursors end up
    /// after the inserted text.
    pub fn insert_at(
        &mut self,
        selections: &[Selection],
        text: &str,
    ) -> Result<EditOutcome, EditError> {
        let sels = normalize(selections);
        let edits: Vec<(std::ops::Range<usize>, &str)> =
            sels.iter().map(|s| (s.range(), text)).collect();
        let after = normalize(&self.cursors_after_replacements(&sels, text.len()));
        let edit = self.edit_with_selections(&edits, &sels, &after)?;
        Ok(EditOutcome {
            edit,
            selections: after,
        })
    }

    /// Delete the selections; an empty selection deletes the previous
    /// grapheme (no-op at the start of the buffer).
    pub fn backspace(&mut self, selections: &[Selection]) -> Result<EditOutcome, EditError> {
        let before = normalize(selections);
        let ranges: Vec<Selection> = before
            .iter()
            .map(|s| {
                if s.is_empty() {
                    Selection::new(self.prev_grapheme_boundary(s.head), s.head)
                } else {
                    *s
                }
            })
            .collect();
        self.delete_ranges(&ranges, &before)
    }

    /// Delete the selections; an empty selection deletes the next grapheme.
    pub fn delete_forward(&mut self, selections: &[Selection]) -> Result<EditOutcome, EditError> {
        let before = normalize(selections);
        let ranges: Vec<Selection> = before
            .iter()
            .map(|s| {
                if s.is_empty() {
                    Selection::new(s.head, self.next_grapheme_boundary(s.head))
                } else {
                    *s
                }
            })
            .collect();
        self.delete_ranges(&ranges, &before)
    }

    /// Text covered by the selections, joined with `\n` (clipboard copy).
    pub fn selected_text(&self, selections: &[Selection]) -> Result<String, EditError> {
        let parts: Result<Vec<String>, EditError> = normalize(selections)
            .iter()
            .filter(|s| !s.is_empty())
            .map(|s| self.slice(s.range()))
            .collect();
        Ok(parts?.join("\n"))
    }

    /// Delete every line touched by a selection (including its line break).
    pub fn delete_lines(&mut self, selections: &[Selection]) -> Result<EditOutcome, EditError> {
        let blocks = self.line_blocks(selections);
        let edits: Vec<(std::ops::Range<usize>, &str)> =
            blocks.iter().map(|(r, _)| (r.clone(), "")).collect();
        let before = normalize(selections);
        // Each cursor lands at the start of where its block was, shifted by
        // earlier deletions.
        let mut after = Vec::with_capacity(blocks.len());
        let mut shift = 0usize;
        for (r, _) in &blocks {
            after.push(Selection::cursor(r.start - shift));
            shift += r.len();
        }
        let edit = self.edit_with_selections(&edits, &before, &after)?;
        Ok(EditOutcome {
            edit,
            selections: after,
        })
    }

    /// Duplicate every line touched by a selection below itself; cursors
    /// move onto the copy.
    pub fn duplicate_lines(&mut self, selections: &[Selection]) -> Result<EditOutcome, EditError> {
        let blocks = self.line_blocks(selections);
        let before = normalize(selections);
        let mut texts: Vec<String> = Vec::with_capacity(blocks.len());
        for (r, has_break) in &blocks {
            let mut t = self.slice(r.clone())?;
            if !has_break {
                t.insert(0, '\n');
            }
            texts.push(t);
        }
        let edits: Vec<(std::ops::Range<usize>, &str)> = blocks
            .iter()
            .zip(&texts)
            .map(|((r, _), t)| (r.end..r.end, t.as_str()))
            .collect();
        // New cursors: same column on the duplicated block (one block down).
        let mut after = Vec::with_capacity(before.len());
        let mut shift = 0usize;
        let mut bi = 0;
        for s in &before {
            while bi + 1 < blocks.len() && blocks[bi].0.end <= s.start() {
                shift += texts[bi].len();
                bi += 1;
            }
            let block_len = texts[bi].len();
            after.push(Selection::new(
                s.anchor + shift + block_len,
                s.head + shift + block_len,
            ));
        }
        let edit = self.edit_with_selections(&edits, &before, &after)?;
        Ok(EditOutcome {
            edit,
            selections: after,
        })
    }

    /// Move the lines touched by the selections one line up or down. Each
    /// contiguous block of touched lines swaps with its own neighbour
    /// (unselected lines between blocks stay put); blocks already at the
    /// edge do not move. Selections travel with their text.
    pub fn move_lines(
        &mut self,
        selections: &[Selection],
        direction: LineDirection,
    ) -> Result<EditOutcome, EditError> {
        let before = normalize(selections);
        let blocks = self.line_blocks(&before);
        let content_end = self.len();
        let mut edits: Vec<(std::ops::Range<usize>, String)> = Vec::new();
        // (block byte range incl. its break, delta for selections inside it)
        let mut deltas: Vec<(std::ops::Range<usize>, isize)> = Vec::new();

        for (block, has_break) in &blocks {
            let block_end = if *has_break {
                self.line_end(block.end.saturating_sub(1))
            } else {
                block.end
            };
            let block_text = self.slice(block.start..block_end)?;
            match direction {
                LineDirection::Up => {
                    if block.start == 0 {
                        continue;
                    }
                    let prev_line = self.point_for_offset(block.start).line - 1;
                    let prev_start = self.line_start_of(prev_line);
                    let prev = self.slice(prev_start..block.start)?; // includes its break
                    let prev_content = prev.trim_end_matches(['\r', '\n']).to_string();
                    let brk = &prev[prev_content.len()..];
                    edits.push((
                        prev_start..block_end,
                        format!("{block_text}{brk}{prev_content}"),
                    ));
                    deltas.push((
                        block.start..block.end.max(block_end + 1),
                        -(prev.len() as isize),
                    ));
                }
                LineDirection::Down => {
                    let next_start = block.end;
                    // Nothing below (last line, or only a phantom empty line
                    // after the trailing break).
                    if !*has_break || next_start >= content_end {
                        continue;
                    }
                    let next_end = self.line_end(next_start);
                    let brk = self.slice(block_end..next_start)?;
                    let next = self.slice(next_start..next_end)?;
                    edits.push((block.start..next_end, format!("{next}{brk}{block_text}")));
                    deltas.push((block.start..block.end, (next.len() + brk.len()) as isize));
                }
            }
        }
        if edits.is_empty() {
            return Ok(EditOutcome {
                edit: None,
                selections: before,
            });
        }
        let after: Vec<Selection> = before
            .iter()
            .map(|s| {
                let delta = deltas
                    .iter()
                    .find(|(r, _)| {
                        r.contains(&s.start()) || (s.start() == r.end && r.end == content_end)
                    })
                    .map(|(_, d)| *d)
                    .unwrap_or(0);
                Selection::new(
                    (s.anchor as isize + delta) as usize,
                    (s.head as isize + delta) as usize,
                )
            })
            .collect();
        let borrowed: Vec<(std::ops::Range<usize>, &str)> =
            edits.iter().map(|(r, t)| (r.clone(), t.as_str())).collect();
        let edit = self.edit_with_selections(&borrowed, &before, &after)?;
        Ok(EditOutcome {
            edit,
            selections: after,
        })
    }

    // ── helpers ─────────────────────────────────────────────────────────

    /// Delete `ranges` (already grapheme-expanded); `before` is the user's
    /// original selection set, recorded for undo.
    fn delete_ranges(
        &mut self,
        ranges: &[Selection],
        before: &[Selection],
    ) -> Result<EditOutcome, EditError> {
        let ranges = normalize(ranges);
        let edits: Vec<(std::ops::Range<usize>, &str)> =
            ranges.iter().map(|s| (s.range(), "")).collect();
        let after = normalize(&self.cursors_after_replacements(&ranges, 0));
        let edit = self.edit_with_selections(&edits, before, &after)?;
        Ok(EditOutcome {
            edit,
            selections: after,
        })
    }

    /// Cursor positions after replacing each (sorted, non-overlapping)
    /// selection with `inserted_len` bytes.
    fn cursors_after_replacements(
        &self,
        sels: &[Selection],
        inserted_len: usize,
    ) -> Vec<Selection> {
        let mut out = Vec::with_capacity(sels.len());
        let mut shift: isize = 0;
        for s in sels {
            let start = (s.start() as isize + shift) as usize;
            out.push(Selection::cursor(start + inserted_len));
            shift += inserted_len as isize - s.range().len() as isize;
        }
        out
    }

    /// Last line a selection touches. A non-empty selection that ends at
    /// column 0 (the classic whole-line shape `"one\n"`) does not touch
    /// the line it ends on.
    fn last_touched_line(&self, s: &Selection) -> usize {
        let end = self.point_for_offset(s.end());
        if !s.is_empty() && end.column == 0 && end.line > 0 {
            end.line - 1
        } else {
            end.line
        }
    }

    /// Whole-line byte blocks covered by the selections, merged, each with
    /// whether it ends in a line break (the last line may not).
    fn line_blocks(&self, selections: &[Selection]) -> Vec<(std::ops::Range<usize>, bool)> {
        let mut blocks: Vec<(std::ops::Range<usize>, bool)> = Vec::new();
        for s in normalize(selections) {
            let first = self.point_for_offset(s.start()).line;
            let last = self.last_touched_line(&s);
            let start = self.offset_for_point(Point {
                line: first,
                column: 0,
            });
            let (end, has_break) = if last + 1 < self.len_lines() {
                (
                    self.offset_for_point(Point {
                        line: last + 1,
                        column: 0,
                    }),
                    true,
                )
            } else {
                (self.len(), false)
            };
            match blocks.last_mut() {
                Some((prev, prev_break)) if start <= prev.end => {
                    prev.end = prev.end.max(end);
                    *prev_break = has_break;
                }
                _ => blocks.push((start..end, has_break)),
            }
        }
        blocks
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempr_domain::SqlFileId;

    fn buf(text: &str) -> Buffer {
        Buffer::new(SqlFileId::new(), text)
    }
    fn text(b: &Buffer) -> String {
        b.text().to_string()
    }

    #[test]
    fn insert_at_multiple_cursors_places_cursors_after_text() {
        let mut b = buf("ab\ncd");
        let out = b
            .insert_at(&[Selection::cursor(0), Selection::cursor(3)], "X")
            .unwrap();
        assert_eq!(text(&b), "Xab\nXcd");
        assert_eq!(
            out.selections,
            vec![Selection::cursor(1), Selection::cursor(5)]
        );
        // Replacing a selection.
        let out = b.insert_at(&[Selection::new(1, 3)], "Q").unwrap();
        assert_eq!(text(&b), "XQ\nXcd");
        assert_eq!(out.selections, vec![Selection::cursor(2)]);
    }

    #[test]
    fn backspace_and_delete_forward() {
        let mut b = buf("héllo");
        let out = b.backspace(&[Selection::cursor(3)]).unwrap(); // after é
        assert_eq!(text(&b), "hllo");
        assert_eq!(out.selections, vec![Selection::cursor(1)]);
        let out = b.backspace(&[Selection::cursor(0)]).unwrap();
        assert_eq!(out.edit, None, "no-op at start");
        assert_eq!(out.selections, vec![Selection::cursor(0)]);
        let out = b.delete_forward(&[Selection::cursor(0)]).unwrap();
        assert_eq!(text(&b), "llo");
        assert_eq!(out.selections, vec![Selection::cursor(0)]);
        let out = b.delete_forward(&[Selection::new(0, 2)]).unwrap();
        assert_eq!(text(&b), "o");
        assert_eq!(out.selections, vec![Selection::cursor(0)]);
    }

    #[test]
    fn selected_text_joins_ranges() {
        let b = buf("select 1;\nselect 2;");
        let s = b
            .selected_text(&[
                Selection::new(0, 6),
                Selection::new(10, 16),
                Selection::cursor(3),
            ])
            .unwrap();
        assert_eq!(s, "select\nselect");
    }

    #[test]
    fn delete_and_duplicate_lines() {
        let mut b = buf("a\nb\nc");
        let out = b.delete_lines(&[Selection::cursor(2)]).unwrap();
        assert_eq!(text(&b), "a\nc");
        assert_eq!(out.selections, vec![Selection::cursor(2)]);
        let out = b.delete_lines(&[Selection::cursor(3)]).unwrap(); // last line, no break
        assert_eq!(text(&b), "a\n");
        assert_eq!(out.selections, vec![Selection::cursor(2)]);

        let mut b = buf("a\nb");
        let out = b.duplicate_lines(&[Selection::cursor(0)]).unwrap();
        assert_eq!(text(&b), "a\na\nb");
        assert_eq!(
            out.selections,
            vec![Selection::cursor(2)],
            "cursor on the copy"
        );
        let out = b.duplicate_lines(&[Selection::cursor(5)]).unwrap(); // last line
        assert_eq!(text(&b), "a\na\nb\nb");
        assert_eq!(out.selections, vec![Selection::cursor(7)]);
    }

    #[test]
    fn move_lines_up_and_down_with_edges() {
        let mut b = buf("one\ntwo\nthree");
        let out = b
            .move_lines(&[Selection::cursor(5)], LineDirection::Up)
            .unwrap();
        assert_eq!(text(&b), "two\none\nthree");
        assert_eq!(out.selections, vec![Selection::cursor(1)]);
        let out = b
            .move_lines(&[Selection::cursor(1)], LineDirection::Up)
            .unwrap();
        assert_eq!(out.edit, None, "already at the top");
        let out = b
            .move_lines(&[Selection::cursor(1)], LineDirection::Down)
            .unwrap();
        assert_eq!(text(&b), "one\ntwo\nthree");
        assert_eq!(out.selections, vec![Selection::cursor(5)]);
        let out = b
            .move_lines(&[Selection::cursor(5)], LineDirection::Down)
            .unwrap();
        assert_eq!(text(&b), "one\nthree\ntwo");
        assert_eq!(out.selections, vec![Selection::cursor(11)]);
        let out = b
            .move_lines(&[Selection::cursor(11)], LineDirection::Down)
            .unwrap();
        assert_eq!(out.edit, None, "already at the bottom");
        // CRLF: breaks travel with the layout, not the content.
        let mut c = buf("a\r\nb");
        c.move_lines(&[Selection::cursor(0)], LineDirection::Down)
            .unwrap();
        assert_eq!(text(&c), "b\r\na");
    }

    #[test]
    fn whole_line_selection_ending_at_column_zero_does_not_touch_next_line() {
        let sel = [Selection::new(0, 4)]; // exactly "one\n"
        let mut b = buf("one\ntwo\nthree");
        b.delete_lines(&sel).unwrap();
        assert_eq!(text(&b), "two\nthree");
        let mut b = buf("one\ntwo\nthree");
        b.duplicate_lines(&sel).unwrap();
        assert_eq!(text(&b), "one\none\ntwo\nthree");
        let mut b = buf("one\ntwo\nthree");
        b.move_lines(&sel, LineDirection::Down).unwrap();
        assert_eq!(text(&b), "two\none\nthree");
    }

    #[test]
    fn move_lines_handles_trailing_newline_and_separate_blocks() {
        let mut b = buf("a\nb\n");
        let out = b
            .move_lines(&[Selection::cursor(2)], LineDirection::Down)
            .unwrap();
        assert_eq!(out.edit, None, "last content line stays put");
        assert_eq!(text(&b), "a\nb\n");

        let mut b = buf("l0\nl1\nl2\nl3\nl4\nl5");
        let out = b
            .move_lines(
                &[Selection::cursor(0), Selection::cursor(12)],
                LineDirection::Down,
            )
            .unwrap();
        assert_eq!(text(&b), "l1\nl0\nl2\nl3\nl5\nl4");
        assert_eq!(
            out.selections,
            vec![Selection::cursor(3), Selection::cursor(15)]
        );
        let out = b
            .move_lines(
                &[Selection::cursor(3), Selection::cursor(15)],
                LineDirection::Up,
            )
            .unwrap();
        assert_eq!(text(&b), "l0\nl1\nl2\nl3\nl4\nl5");
        assert_eq!(
            out.selections,
            vec![Selection::cursor(0), Selection::cursor(12)]
        );
    }

    #[test]
    fn backspace_records_original_cursor_and_normalizes() {
        let mut b = buf("héllo");
        b.backspace(&[Selection::cursor(3)]).unwrap();
        let (_, sel) = b.undo_with_selections().unwrap();
        assert_eq!(sel, Some(vec![Selection::cursor(3)]));
        let mut b = buf("abc");
        let out = b
            .backspace(&[Selection::cursor(1), Selection::cursor(2)])
            .unwrap();
        assert_eq!(text(&b), "c");
        assert_eq!(out.selections, vec![Selection::cursor(0)]);
    }

    #[test]
    fn undo_and_redo_restore_selections() {
        let mut b = buf("ab");
        let before = vec![Selection::cursor(1)];
        let out = b.insert_at(&before, "XYZ").unwrap();
        assert_eq!(text(&b), "aXYZb");
        let (_, sel) = b.undo_with_selections().unwrap();
        assert_eq!(text(&b), "ab");
        assert_eq!(sel, Some(before));
        let (_, sel) = b.redo_with_selections().unwrap();
        assert_eq!(text(&b), "aXYZb");
        assert_eq!(sel, Some(out.selections));
        // Plain edits record no selections.
        b.edit(&[(0..0, "!")]).unwrap();
        assert_eq!(b.undo_with_selections().unwrap().1, None);
    }
}
