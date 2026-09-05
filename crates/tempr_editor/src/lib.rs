//! `tempr_editor` — the SQL editor model layer (docs/10-editor.md).
//!
//! Plain Rust, no GPUI: testable in isolation. `Buffer` owns the text (a
//! `ropey` rope) and the edit history; the syntax tree and statement detector
//! arrive in follow-up tasks.

#![deny(unsafe_code)]
#![cfg_attr(test, allow(clippy::expect_used, clippy::unwrap_used))]

pub mod buffer;
pub mod edit_ops;
pub mod motion;
pub mod selection;
pub mod syntax;

pub use buffer::{Buffer, EditError, EditId, Point};
pub use edit_ops::EditOutcome;
pub use selection::Selection;
pub use syntax::{Highlight, HighlightKind, StatementKind, StatementRange, SyntaxTree};
