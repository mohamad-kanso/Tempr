//! Completion popup: GPUI actions, the popup's key context and its state
//! (docs/11-gpui.md → Component Library Catalog, spec §7).
//!
//! Every action is bound under [`KEY_CONTEXT`], which exists only while the
//! popup is open; the bindings are `.hidden()` in the command catalog, so a
//! closed popup can never fire `completion::Accept` by key or from palette
//! search.

use gpui::actions;
use tempr_intel::{CompletionItem, CompletionKind};

use std::ops::Range;

actions!(completion, [Next, Prev, Accept, Dismiss]);

/// Key context while the popup is open: `Editor` plus `Completion`, so editor
/// bindings still fire and completion bindings outrank them.
pub const KEY_CONTEXT: &str = "Editor Completion";
/// Predicate for the catalog: both identifiers live on the focused editor
/// node while the popup is open. Every command carrying this is hidden from
/// palette search.
pub const BINDING_CONTEXT: &str = "Editor && Completion";

/// The replacement a chosen item performs: its `replace` range and the caret
/// offset after the inserted text.
pub fn accept_edit(item: &CompletionItem) -> (Range<usize>, usize) {
    let cursor = item.replace.start + item.insert.len();
    (item.replace.clone(), cursor)
}

/// The popup's rows and the highlighted index into them.
#[derive(Debug, Default)]
pub struct CompletionPopup {
    pub items: Vec<CompletionItem>,
    pub selected: usize,
}

impl CompletionPopup {
    pub fn new(items: Vec<CompletionItem>) -> Self {
        Self { items, selected: 0 }
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn selected_item(&self) -> Option<&CompletionItem> {
        self.items.get(self.selected)
    }

    /// Move the selection by `delta`, wrapping at both ends.
    pub fn move_selection(&mut self, delta: isize) {
        if self.items.is_empty() {
            self.selected = 0;
            return;
        }
        let len = self.items.len() as isize;
        self.selected = (self.selected as isize + delta).rem_euclid(len) as usize;
    }

    /// Keep the current item selected after a re-filter when it is still
    /// present; otherwise select the first row.
    pub fn refilter(&mut self, items: Vec<CompletionItem>) {
        let previous = self
            .selected_item()
            .map(|item| (item.label.clone(), item.kind));
        self.items = items;
        self.selected = previous
            .and_then(|(label, kind)| {
                self.items
                    .iter()
                    .position(|item| item.label == label && item.kind == kind)
            })
            .unwrap_or(0);
    }

    pub fn kind_label(kind: CompletionKind) -> &'static str {
        match kind {
            CompletionKind::Keyword => "kw",
            CompletionKind::Table => "table",
            CompletionKind::Column => "col",
            CompletionKind::Schema => "schema",
            CompletionKind::Alias => "alias",
            CompletionKind::Cte => "cte",
            CompletionKind::Function => "fn",
            CompletionKind::Snippet => "snip",
            CompletionKind::Plugin => "plugin",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(label: &str) -> CompletionItem {
        CompletionItem {
            label: label.into(),
            kind: CompletionKind::Column,
            detail: String::new(),
            insert: label.into(),
            replace: 10..12,
            score: 0,
        }
    }

    #[test]
    fn accept_replaces_the_word_and_places_the_caret_after_it() {
        let mut item = item("customer_id");
        item.replace = 7..9;
        let (range, cursor) = accept_edit(&item);
        assert_eq!(range, 7..9);
        assert_eq!(cursor, 7 + "customer_id".len());
        assert_eq!(&"select xy from users"[range.clone()], "xy");
    }

    #[test]
    fn accept_after_a_qualifier_inserts_at_the_cursor() {
        let mut item = item("id");
        item.replace = 20..20;
        let (range, cursor) = accept_edit(&item);
        assert!(range.is_empty());
        assert_eq!(cursor, 22);
    }

    #[test]
    fn selection_wraps_at_both_ends() {
        let mut popup = CompletionPopup::new(vec![item("a"), item("b"), item("c")]);
        popup.move_selection(-1);
        assert_eq!(popup.selected, 2, "up from the first wraps to the last");
        popup.move_selection(1);
        assert_eq!(popup.selected, 0, "down from the last wraps to the first");
    }

    #[test]
    fn refilter_keeps_the_selected_label_when_it_survives() {
        let mut popup = CompletionPopup::new(vec![item("alpha"), item("beta")]);
        popup.move_selection(1);
        popup.refilter(vec![item("beta"), item("gamma")]);
        assert_eq!(popup.selected, 0, "beta stays selected");
        popup.refilter(vec![item("gamma")]);
        assert_eq!(popup.selected, 0, "gone selection clamps to the first row");
    }

    #[test]
    fn refilter_does_not_confuse_items_that_share_a_label() {
        let mut keyword = item("id");
        keyword.kind = CompletionKind::Keyword;
        let mut popup = CompletionPopup::new(vec![item("id"), keyword.clone()]);
        popup.move_selection(1);
        let mut column = item("id");
        column.kind = CompletionKind::Column;
        popup.refilter(vec![column, keyword]);
        assert_eq!(
            popup.selected, 1,
            "the keyword row stays selected, not the first same-label column"
        );
    }
}
