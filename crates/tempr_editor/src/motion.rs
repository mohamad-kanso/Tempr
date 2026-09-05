//! Cursor motions over a `Buffer`, all in byte offsets and all returning a
//! char-boundary offset. Grapheme and word boundaries use
//! `unicode-segmentation` on the current line (lines are short; the rope
//! keeps everything else O(log n)).

use unicode_segmentation::UnicodeSegmentation;

use crate::buffer::{Buffer, Point};

impl Buffer {
    /// Byte offset of the start of line `line` (clamped to the last line).
    pub fn line_start_of(&self, line: usize) -> usize {
        self.offset_for_point(Point { line, column: 0 })
    }

    /// Byte offset of the end of line `line`'s content (before its break).
    pub fn line_end_of(&self, line: usize) -> usize {
        self.offset_for_point(Point {
            line,
            column: usize::MAX,
        })
    }

    /// Byte offset of the start of the line containing `offset`.
    pub fn line_start(&self, offset: usize) -> usize {
        let p = self.point_for_offset(offset);
        self.offset_for_point(Point {
            line: p.line,
            column: 0,
        })
    }

    /// Byte offset just before the line break of the line containing `offset`.
    pub fn line_end(&self, offset: usize) -> usize {
        let p = self.point_for_offset(offset);
        self.offset_for_point(Point {
            line: p.line,
            column: usize::MAX,
        })
    }

    /// Next grapheme boundary after `offset`; crosses line breaks; clamps.
    pub fn next_grapheme_boundary(&self, offset: usize) -> usize {
        let offset = offset.min(self.len());
        if offset == self.len() {
            return offset;
        }
        let end = self.line_end(offset);
        if offset >= end {
            // On the line break: step to the next line's start.
            let p = self.point_for_offset(offset);
            return if p.line + 1 < self.len_lines() {
                self.offset_for_point(Point {
                    line: p.line + 1,
                    column: 0,
                })
            } else {
                self.len()
            };
        }
        let start = self.line_start(offset);
        let line = self.line_text(offset);
        let rel = offset - start;
        line.grapheme_indices(true)
            .map(|(i, g)| i + g.len())
            .find(|&e| e > rel)
            .map(|e| start + e)
            .unwrap_or(end)
    }

    /// Previous grapheme boundary before `offset`; crosses line breaks.
    pub fn prev_grapheme_boundary(&self, offset: usize) -> usize {
        let offset = offset.min(self.len());
        if offset == 0 {
            return 0;
        }
        let start = self.line_start(offset);
        if offset <= start {
            // At a line start: step to the previous line's end.
            let p = self.point_for_offset(offset);
            return self.offset_for_point(Point {
                line: p.line - 1,
                column: usize::MAX,
            });
        }
        let line = self.line_text(offset);
        let rel = offset - start;
        line.grapheme_indices(true)
            .map(|(i, _)| i)
            .rev()
            .find(|&i| i < rel)
            .map(|i| start + i)
            .unwrap_or(start)
    }

    /// End of the word at/after `offset` (Unicode word boundaries; runs of
    /// whitespace are skipped). At a line end, moves to the next line start.
    pub fn next_word_boundary(&self, offset: usize) -> usize {
        let offset = offset.min(self.len());
        let end = self.line_end(offset);
        if offset >= end {
            return self.next_grapheme_boundary(offset);
        }
        let start = self.line_start(offset);
        let line = self.line_text(offset);
        let rel = offset - start;
        for (i, w) in line.split_word_bound_indices() {
            let w_end = i + w.len();
            if w_end <= rel || w.trim().is_empty() {
                continue;
            }
            return start + w_end;
        }
        end
    }

    /// Start of the word at/before `offset`. At a line start, moves to the
    /// previous line's end.
    pub fn prev_word_boundary(&self, offset: usize) -> usize {
        let offset = offset.min(self.len());
        let start = self.line_start(offset);
        if offset <= start {
            return self.prev_grapheme_boundary(offset);
        }
        let line = self.line_text(offset);
        let rel = offset - start;
        let mut result = start;
        for (i, w) in line.split_word_bound_indices() {
            if i >= rel {
                break;
            }
            if !w.trim().is_empty() {
                result = start + i;
            }
        }
        result
    }

    /// Move `delta` lines from `offset`, keeping `goal_column` (bytes) when
    /// given, else the current column. Returns the new offset and the goal
    /// column to carry into the next vertical move.
    pub fn move_vertically(
        &self,
        offset: usize,
        delta: isize,
        goal_column: Option<usize>,
    ) -> (usize, usize) {
        let p = self.point_for_offset(offset);
        let goal = goal_column.unwrap_or(p.column);
        let last = self.len_lines().saturating_sub(1) as isize;
        let line = (p.line as isize + delta).clamp(0, last) as usize;
        (self.offset_for_point(Point { line, column: goal }), goal)
    }

    /// Content (no line break) of the line containing `offset`.
    fn line_text(&self, offset: usize) -> String {
        let p = self.point_for_offset(offset);
        self.line(p.line).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempr_domain::SqlFileId;

    fn buf(text: &str) -> Buffer {
        Buffer::new(SqlFileId::new(), text)
    }

    #[test]
    fn grapheme_motion_handles_multibyte_and_line_breaks() {
        // "aé\r\nb" : a0 é1-2 \r3 \n4 b5
        let b = buf("aé\r\nb");
        assert_eq!(b.next_grapheme_boundary(0), 1);
        assert_eq!(b.next_grapheme_boundary(1), 3, "é is two bytes");
        assert_eq!(
            b.next_grapheme_boundary(3),
            5,
            "line break → next line start"
        );
        assert_eq!(b.next_grapheme_boundary(5), 6);
        assert_eq!(b.next_grapheme_boundary(6), 6, "clamps at end");
        assert_eq!(b.prev_grapheme_boundary(6), 5);
        assert_eq!(
            b.prev_grapheme_boundary(5),
            3,
            "line start → previous line end"
        );
        assert_eq!(b.prev_grapheme_boundary(3), 1);
        assert_eq!(b.prev_grapheme_boundary(0), 0);
        // Combining sequences move as one unit.
        let c = buf("e\u{301}x");
        assert_eq!(c.next_grapheme_boundary(0), 3);
        assert_eq!(c.prev_grapheme_boundary(3), 0);
    }

    #[test]
    fn word_motion() {
        let b = buf("select  id_1, name from t\nx");
        assert_eq!(b.next_word_boundary(0), 6, "end of 'select'");
        assert_eq!(b.next_word_boundary(6), 12, "skips spaces to end of 'id_1'");
        assert_eq!(b.next_word_boundary(12), 13, "punctuation is a word");
        assert_eq!(b.prev_word_boundary(12), 8, "start of 'id_1'");
        assert_eq!(b.prev_word_boundary(8), 0);
        let end = b.line_end(0);
        assert_eq!(b.next_word_boundary(end), end + 1, "line end → next line");
        assert_eq!(
            b.prev_word_boundary(end + 1),
            end,
            "line start → previous line end"
        );
    }

    #[test]
    fn line_start_end_and_vertical_goal_column() {
        let b = buf("short\na much longer line\nmid");
        assert_eq!(b.line_start(8), 6);
        assert_eq!(b.line_end(8), 24);
        // From column 15 on line 1, up to line 0 clamps to its end but keeps
        // the goal; down again restores column 15.
        let (o, goal) = b.move_vertically(6 + 15, -1, None);
        assert_eq!((o, goal), (5, 15));
        let (o2, _) = b.move_vertically(o, 1, Some(goal));
        assert_eq!(o2, 6 + 15);
        let (o3, _) = b.move_vertically(o2, 5, Some(goal));
        assert_eq!(o3, b.len(), "clamps to the last line's end");
        let (o4, _) = b.move_vertically(o3, -9, Some(goal));
        assert_eq!(o4, 5);
    }
}
