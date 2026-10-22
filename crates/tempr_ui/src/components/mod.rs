//! Tempr-authored components (docs/11-gpui.md → Component Library Catalog).
//! GPUI ships no text input and no data grid; both live here.

pub mod completion_popup;
pub mod editor_view;
pub mod input;
pub mod palette;
pub mod result_grid;

pub use completion_popup::CompletionPopup;
pub use editor_view::{DiagnosticsSummary, EditorEvent, EditorView};
pub use input::{Input, InputEvent};
pub use palette::{Palette, PaletteEvent};
pub use result_grid::ResultGrid;
