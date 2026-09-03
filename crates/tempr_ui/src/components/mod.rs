//! Tempr-authored components (docs/11-gpui.md → Component Library Catalog).
//! GPUI ships no text input and no data grid; both live here.

pub mod input;
pub mod results_grid;

pub use input::{Input, InputEvent};
pub use results_grid::ResultsGrid;
