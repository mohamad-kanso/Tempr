//! Completion popup: GPUI actions, the popup's key context and its state
//! (docs/11-gpui.md → Component Library Catalog, spec §7).
//!
//! The popup is rendered by `EditorView` next to the caret; this module owns
//! the action types and the row model so the binding catalog and the view
//! share one definition.
//!
//! Every action is bound under [`KEY_CONTEXT`]. That context exists only
//! while the popup is open — the bindings are `.hidden()` in the command
//! catalog, so a closed popup can never fire `completion::Accept` by key or
//! from palette search.

use gpui::actions;
use tempr_intel::{CompletionItem, CompletionKind};

actions!(completion, [Next, Prev, Accept, Dismiss]);

/// Key context added by the editor while the popup is open.
pub const KEY_CONTEXT: &str = "Editor Completion";

/// The popup's visible rows are `items`, with `selected` as the highlighted
/// index. Kept separate from `CompletionItem` so re-filtering after a
/// keystroke can preserve (or clamp) the selection without touching the
/// engine's ranking.
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
    /// present; otherwise fall back to the first row.
    pub fn refilter(&mut self, items: Vec<CompletionItem>) {
        let previous = self.selected_item().map(|item| item.label.clone());
        self.items = items;
        self.selected = previous
            .and_then(|label| self.items.iter().position(|item| item.label == label))
            .unwrap_or(0);
    }

    /// The label to show for a kind, in the width the popup reserves.
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
