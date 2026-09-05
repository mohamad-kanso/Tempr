//! A cursor with an optional selection, in byte offsets.

use std::ops::Range;

/// `head` is where the cursor is; `anchor` is where the selection started.
/// `anchor == head` means no selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Selection {
    pub anchor: usize,
    pub head: usize,
}

impl Selection {
    pub fn cursor(offset: usize) -> Self {
        Self {
            anchor: offset,
            head: offset,
        }
    }

    pub fn new(anchor: usize, head: usize) -> Self {
        Self { anchor, head }
    }

    pub fn is_empty(&self) -> bool {
        self.anchor == self.head
    }

    pub fn start(&self) -> usize {
        self.anchor.min(self.head)
    }

    pub fn end(&self) -> usize {
        self.anchor.max(self.head)
    }

    pub fn range(&self) -> Range<usize> {
        self.start()..self.end()
    }

    /// Move the head; keep the anchor when `extend`, else collapse onto it.
    pub fn with_head(&self, head: usize, extend: bool) -> Self {
        Self {
            anchor: if extend { self.anchor } else { head },
            head,
        }
    }

    pub fn collapsed_to_start(&self) -> Self {
        Self::cursor(self.start())
    }

    pub fn collapsed_to_end(&self) -> Self {
        Self::cursor(self.end())
    }
}

/// Sort by start and merge overlapping or touching selections (multi-cursor
/// edits must not overlap). Anchors of merged selections point at the start
/// unless the selection being kept was reversed.
pub fn normalize(selections: &[Selection]) -> Vec<Selection> {
    let mut sorted: Vec<Selection> = selections.to_vec();
    sorted.sort_by_key(|s| (s.start(), s.end()));
    let mut out: Vec<Selection> = Vec::with_capacity(sorted.len());
    for s in sorted {
        match out.last_mut() {
            // Two empty cursors at the same spot collapse; ranges that
            // overlap (not merely touch, unless one is empty) merge.
            Some(last)
                if s.start() < last.end()
                    || (s.is_empty() && s.start() == last.end() && last.is_empty()) =>
            {
                let start = last.start();
                let end = last.end().max(s.end());
                let reversed = last.head < last.anchor;
                *last = if reversed {
                    Selection::new(end, start)
                } else {
                    Selection::new(start, end)
                };
            }
            _ => out.push(s),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basics() {
        let s = Selection::new(5, 2);
        assert_eq!((s.start(), s.end()), (2, 5));
        assert_eq!(s.range(), 2..5);
        assert!(!s.is_empty());
        assert_eq!(s.with_head(7, false), Selection::cursor(7));
        assert_eq!(s.with_head(7, true), Selection::new(5, 7));
        assert_eq!(s.collapsed_to_start(), Selection::cursor(2));
    }

    #[test]
    fn normalize_sorts_and_merges() {
        let v = normalize(&[
            Selection::new(10, 12),
            Selection::cursor(3),
            Selection::new(4, 2),
            Selection::new(11, 15),
            Selection::cursor(3),
        ]);
        // The cursor at 3 sits inside 2..4 and is absorbed.
        assert_eq!(v, vec![Selection::new(4, 2), Selection::new(10, 15)]);
        // Touching non-empty ranges stay separate.
        assert_eq!(
            normalize(&[Selection::new(0, 2), Selection::new(2, 4)]).len(),
            2
        );
    }
}
