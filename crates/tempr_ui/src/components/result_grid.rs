//! Virtualized result grid: fixed-height rows via `uniform_list`, so a
//! 100,000-row result set allocates elements for the visible rows only.
//!
//! Owns the rows it displays (Phase 1: `Vec<Vec<Value>>`; the columnar
//! `RowStore` with spill-to-disk from docs/13-result-grid.md is a follow-up).
//! Rows arrive as `Batch`es through [`ResultGrid::append`]; the parent view
//! calls `cx.notify()` after each append so the grid fills incrementally.

use gpui::{
    AnyElement, Context, IntoElement, Render, ScrollStrategy, UniformListScrollHandle, Window, div,
    prelude::*, px, rgb, uniform_list,
};
use tempr_domain::{Batch, ColumnSpec, Value};

use crate::theme;
use crate::value_format::format_value;

pub const ROW_HEIGHT: f32 = 24.0;
pub const HEADER_HEIGHT: f32 = 28.0;
pub const COLUMN_WIDTH: f32 = 180.0;

pub struct ResultGrid {
    columns: Vec<ColumnSpec>,
    rows: Vec<Vec<Value>>,
    /// Shown instead of the table when there are no columns yet.
    message: Option<String>,
    scroll_handle: UniformListScrollHandle,
    /// Row range rendered by the last frame (diagnostics: proves the
    /// viewport moved during the scroll benchmark).
    last_rendered: std::ops::Range<usize>,
}

impl Default for ResultGrid {
    fn default() -> Self {
        Self::new()
    }
}

impl ResultGrid {
    pub fn new() -> Self {
        Self {
            columns: Vec::new(),
            rows: Vec::new(),
            message: Some("Run a query (Enter or ctrl-enter) to see results here.".into()),
            scroll_handle: UniformListScrollHandle::new(),
            last_rendered: 0..0,
        }
    }

    /// Row range the last frame rendered.
    pub fn last_rendered_rows(&self) -> std::ops::Range<usize> {
        self.last_rendered.clone()
    }

    /// Scroll so that `row` is the first visible row (clamped by the list).
    /// Strict: moves even when `row` is already visible.
    pub fn scroll_to_row(&self, row: usize) {
        self.scroll_handle
            .scroll_to_item_strict(row, ScrollStrategy::Top);
    }

    /// Start a new result set: drop rows, keep the grid empty until columns.
    pub fn reset(&mut self) {
        self.columns.clear();
        self.rows.clear();
        self.message = None;
    }

    pub fn set_columns(&mut self, columns: Vec<ColumnSpec>) {
        self.columns = columns;
        self.rows.clear();
        self.message = None;
    }

    pub fn append(&mut self, batch: Batch) {
        self.rows.extend(batch.rows);
    }

    pub fn set_message(&mut self, message: impl Into<String>) {
        self.message = Some(message.into());
    }

    pub fn row_count(&self) -> usize {
        self.rows.len()
    }

    pub fn column_count(&self) -> usize {
        self.columns.len()
    }

    fn total_width(&self) -> f32 {
        (self.columns.len().max(1) as f32) * COLUMN_WIDTH
    }

    fn render_cell(text: String, is_null: bool) -> impl IntoElement {
        div()
            .w(px(COLUMN_WIDTH))
            .h_full()
            .px(px(8.))
            .flex()
            .items_center()
            .overflow_hidden()
            .whitespace_nowrap()
            .border_r_1()
            .border_color(rgb(theme::BORDER))
            .text_color(rgb(if is_null {
                theme::TEXT_DIM
            } else {
                theme::TEXT
            }))
            .child(text)
    }

    fn render_row(&self, ix: usize) -> AnyElement {
        let row = &self.rows[ix];
        let bg = if ix.is_multiple_of(2) {
            theme::SURFACE
        } else {
            theme::SURFACE_RAISED
        };
        div()
            .id(ix)
            .flex()
            .h(px(ROW_HEIGHT))
            .w(px(self.total_width()))
            .bg(rgb(bg))
            .children(row.iter().map(|v| {
                let is_null = matches!(v, Value::Null);
                Self::render_cell(format_value(v), is_null)
            }))
            .into_any_element()
    }

    fn render_header(&self) -> impl IntoElement {
        div()
            .flex()
            .h(px(HEADER_HEIGHT))
            .w(px(self.total_width()))
            .bg(rgb(theme::SURFACE_RAISED))
            .border_b_1()
            .border_color(rgb(theme::BORDER))
            .text_color(rgb(theme::ACCENT))
            .children(self.columns.iter().map(|c| {
                div()
                    .w(px(COLUMN_WIDTH))
                    .h_full()
                    .px(px(8.))
                    .flex()
                    .items_center()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .border_r_1()
                    .border_color(rgb(theme::BORDER))
                    .child(format!("{} · {}", c.name, c.data_type))
            }))
    }
}

impl Render for ResultGrid {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.columns.is_empty() {
            return div()
                .size_full()
                .p_3()
                .text_color(rgb(theme::TEXT_DIM))
                .child(self.message.clone().unwrap_or_default())
                .into_any_element();
        }

        let row_count = self.rows.len();
        div()
            .id("results-scroll")
            .size_full()
            .overflow_x_scroll()
            // Without this, gpui's cross-axis fallback turns every vertical
            // wheel tick into horizontal scroll here (this container has no
            // vertical overflow), so the grid slides sideways instead of down.
            .restrict_scroll_to_axis()
            .text_size(px(13.))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .h_full()
                    // A narrow result set must still fill the viewport so the
                    // wheel scrolls rows anywhere in the grid, not only over them.
                    .min_w_full()
                    .w(px(self.total_width()))
                    .child(self.render_header())
                    .child(div().flex_1().min_h_0().child({
                        let mut rows = uniform_list(
                            "result-rows",
                            row_count,
                            cx.processor(|this, range: std::ops::Range<usize>, _window, _cx| {
                                this.last_rendered = range.clone();
                                range.map(|ix| this.render_row(ix)).collect::<Vec<_>>()
                            }),
                        )
                        .track_scroll(&self.scroll_handle)
                        .h_full();
                        // The mirror of the container's rule: a horizontal wheel
                        // (shift-wheel, tilt) must reach the container only, not fall
                        // back to scrolling rows vertically. `restrict_scroll_to_axis`
                        // is not exposed on `UniformList`, hence the style field.
                        rows.interactivity().base_style.restrict_scroll_to_axis = Some(true);
                        rows
                    })),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn col(name: &str) -> ColumnSpec {
        ColumnSpec {
            name: name.into(),
            ordinal: 0,
            data_type: "int8".into(),
            value_type: tempr_domain::ValueType::Int,
            nullable: true,
            table_schema: None,
            table_name: None,
        }
    }

    #[test]
    fn append_accumulates_and_reset_clears() {
        let mut g = ResultGrid::new();
        g.set_columns(vec![col("a"), col("b")]);
        g.append(Batch {
            rows: vec![vec![Value::Int8(1), Value::Null]],
            batch_index: 0,
        });
        g.append(Batch {
            rows: vec![vec![Value::Int8(2), Value::Null]; 3],
            batch_index: 1,
        });
        assert_eq!(g.row_count(), 4);
        assert_eq!(g.column_count(), 2);
        assert_eq!(g.total_width(), 2.0 * COLUMN_WIDTH);
        g.reset();
        assert_eq!(g.row_count(), 0);
        assert_eq!(g.column_count(), 0);
    }
}
