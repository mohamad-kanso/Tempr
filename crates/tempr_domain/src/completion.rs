//! Completion items and the plugin-facing completion request
//! (docs/12-sql-intelligence.md §5.2, docs/08-plugin-api.md).
//!
//! These types live in the domain because both the internal engine
//! (`tempr_intel`) and the plugin API (`tempr_plugin`) produce and consume
//! them, and neither may depend on the other. The engine's own
//! `CompletionRequest` stays in `tempr_intel`: it borrows the editor buffer
//! mutably for a lazy reparse, and `tempr_domain` must not depend on
//! `tempr_editor`.

use std::ops::Range;

use serde::{Deserialize, Serialize};

use crate::ids::{ConnectionId, SqlFileId};

/// Category for the completion popup's icon/grouping.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompletionKind {
    Keyword,
    Table,
    Column,
    Schema,
    Alias,
    Cte,
    Function,
    Snippet,
    /// Contributed by a plugin through `CompletionProvider`.
    Plugin,
}

/// One ranked candidate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompletionItem {
    /// Text shown in the popup.
    pub label: String,
    pub kind: CompletionKind,
    /// Type, signature, schema or ownership hint (empty when there is none).
    pub detail: String,
    /// Text the editor writes. Equal to `label` today; separate so snippets
    /// can differ without reshaping the item.
    pub insert: String,
    /// Byte range in the buffer the item replaces (the word under the cursor;
    /// empty when the cursor follows a dot or whitespace).
    pub replace: Range<usize>,
    /// Ranking score, highest first.
    pub score: i32,
}

impl CompletionItem {
    /// A plugin-contributed item. `priority` is a weight hint in `0..=300`:
    /// the host maps it to `WEIGHT_PLUGIN_BASE (100) + priority`, so a plugin
    /// can rank between keywords (100) and statement keywords (400) but never
    /// above a schema object (450). It is stored in `score` because the final
    /// ranking score is the host's to compute.
    pub fn plugin(
        label: impl Into<String>,
        insert: impl Into<String>,
        detail: impl Into<String>,
        priority: i32,
        replace: Range<usize>,
    ) -> Self {
        Self {
            label: label.into(),
            kind: CompletionKind::Plugin,
            detail: detail.into(),
            insert: insert.into(),
            replace,
            score: priority.clamp(0, 300),
        }
    }
}

/// What a plugin's `CompletionProvider` may see. Deliberately text-based:
/// exposing the engine's `Analysis` or the editor's `Buffer` would freeze
/// internal types into the public API.
#[derive(Debug, Clone)]
pub struct PluginCompletionRequest<'a> {
    pub file: SqlFileId,
    /// The buffer's text at request time.
    pub text: &'a str,
    /// Byte offset of the cursor within `text`.
    pub offset: usize,
    /// The identifier fragment before the cursor (empty after a dot or
    /// whitespace), exactly as typed — providers decide their own
    /// case-insensitivity. Use [`Self::replace_range`] to build item ranges.
    pub prefix: &'a str,
    /// True for `ctrl-space`: the user asked explicitly.
    pub explicit: bool,
    pub connection: Option<ConnectionId>,
}

impl PluginCompletionRequest<'_> {
    /// The byte range a returned item should replace: the prefix under the
    /// cursor.
    pub fn replace_range(&self) -> Range<usize> {
        self.offset.saturating_sub(self.prefix.len())..self.offset
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plugin_items_carry_the_plugin_kind_and_clamped_priority() {
        let item = CompletionItem::plugin("demo_x", "demo_x()", "snippet", 40, 10..14);
        assert_eq!(item.kind, CompletionKind::Plugin);
        assert_eq!(item.score, 40);
        assert_eq!(CompletionItem::plugin("a", "a", "", 9_000, 0..0).score, 300);
        assert_eq!(CompletionItem::plugin("a", "a", "", -5, 0..0).score, 0);
    }

    #[test]
    fn replace_range_covers_the_prefix() {
        let req = PluginCompletionRequest {
            file: SqlFileId::new(),
            text: "select * from us",
            offset: 16,
            prefix: "us",
            explicit: false,
            connection: None,
        };
        assert_eq!(req.replace_range(), 14..16);
    }

    #[test]
    fn empty_prefix_yields_an_empty_range() {
        let req = PluginCompletionRequest {
            file: SqlFileId::new(),
            text: "select * from u.",
            offset: 15,
            prefix: "",
            explicit: true,
            connection: None,
        };
        assert_eq!(req.replace_range(), 15..15);
    }
}
