//! Virtualized result grid: fixed-height rows via `uniform_list`, so a
//! 100,000-row result set allocates elements for the visible rows only.
//!
//! Owns the rows it displays (Phase 1: `Vec<Vec<Value>>`; the columnar
//! `RowStore` with spill-to-disk from docs/13-result-grid.md is a follow-up).
//! Rows arrive as `Batch`es through [`ResultGrid::append`]; the parent view
//! calls `cx.notify()` after each append so the grid fills incrementally.

use gpui::{
    AnyElement, Context, IntoElement, Render, ScrollDelta, ScrollHandle, ScrollStrategy,
    ScrollWheelEvent, UniformListScrollHandle, Window, div, prelude::*, px, rgb, uniform_list,
};
use tempr_domain::{Batch, ColumnSpec, Value};

use crate::theme;
use crate::value_format::format_value;

pub const ROW_HEIGHT: f32 = 24.0;
pub const HEADER_HEIGHT: f32 = 28.0;
pub const COLUMN_WIDTH: f32 = 180.0;
/// Extra columns rendered past each viewport edge so a horizontal scroll
/// shows content immediately while the next frame renders the new range.
const OVERSCAN_COLUMNS: usize = 2;

pub struct ResultGrid {
    columns: Vec<ColumnSpec>,
    rows: Vec<Vec<Value>>,
    /// Shown instead of the table when there are no columns yet.
    message: Option<String>,
    scroll_handle: UniformListScrollHandle,
    /// Horizontal viewport, used to render only the visible column range.
    column_scroll: ScrollHandle,
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
            column_scroll: ScrollHandle::new(),
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

    /// Columns intersecting the horizontal viewport, including overscan.
    fn visible_columns(&self, viewport_width: f32) -> std::ops::Range<usize> {
        // `ScrollHandle` offsets are non-positive: zero at the left edge, more
        // negative as the content moves left. `visible_column_range` wants the
        // distance scrolled, so flip the sign — clamping the raw offset to zero
        // pinned the window to columns 0..N and left the rest of a scrolled
        // wide table blank.
        let scrolled = (-self.column_scroll.offset().x).max(px(0.));
        visible_column_range(self.columns.len(), scrolled.into(), viewport_width)
    }

    fn column_spacer(columns: usize) -> impl IntoElement {
        div()
            .w(px(columns as f32 * COLUMN_WIDTH))
            .h_full()
            .flex_shrink_0()
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

    fn render_row(&self, ix: usize, visible: std::ops::Range<usize>) -> AnyElement {
        let row = &self.rows[ix];
        let bg = if ix.is_multiple_of(2) {
            theme::SURFACE
        } else {
            theme::SURFACE_RAISED
        };
        let start = visible.start;
        let cells = visible.map(|column| {
            let value = row.get(column);
            let is_null = matches!(value, Some(Value::Null));
            let text = value.map(format_value).unwrap_or_default();
            Self::render_cell(text, is_null)
        });
        div()
            .id(ix)
            .flex()
            .h(px(ROW_HEIGHT))
            .w(px(self.total_width()))
            .bg(rgb(bg))
            .children(
                std::iter::once(Self::column_spacer(start).into_any_element())
                    .chain(cells.map(IntoElement::into_any_element)),
            )
            .into_any_element()
    }

    fn render_header(&self, visible: std::ops::Range<usize>) -> impl IntoElement {
        div()
            .flex()
            .h(px(HEADER_HEIGHT))
            .w(px(self.total_width()))
            .bg(rgb(theme::SURFACE_RAISED))
            .border_b_1()
            .border_color(rgb(theme::BORDER))
            .text_color(rgb(theme::ACCENT))
            .children(
                std::iter::once(Self::column_spacer(visible.start).into_any_element()).chain(
                    visible.map(|column| {
                        let c = &self.columns[column];
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
                            .into_any_element()
                    }),
                ),
            )
    }
}

/// The column range that intersects a horizontal viewport, clamped to the
/// table. `offset` is the distance scrolled from the left edge (a non-negative
/// pixel count) and `viewport_width` a pixel count.
fn visible_column_range(
    column_count: usize,
    offset: f32,
    viewport_width: f32,
) -> std::ops::Range<usize> {
    if column_count == 0 {
        return 0..0;
    }
    let offset = offset.max(0.0);
    let viewport_width = viewport_width.max(COLUMN_WIDTH);
    let first = ((offset / COLUMN_WIDTH).floor() as usize).min(column_count - 1);
    let visible = ((viewport_width / COLUMN_WIDTH).ceil() as usize).max(1);
    let start = first.saturating_sub(OVERSCAN_COLUMNS);
    let end = (first + visible + OVERSCAN_COLUMNS).min(column_count);
    start..end
}

impl Render for ResultGrid {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.columns.is_empty() {
            return div()
                .size_full()
                .p_3()
                .text_color(rgb(theme::TEXT_DIM))
                .child(self.message.clone().unwrap_or_default())
                .into_any_element();
        }

        let bounds_width: f32 = self.column_scroll.bounds().size.width.into();
        let viewport_width = if bounds_width > 0.0 {
            bounds_width
        } else {
            window.viewport_size().width.into()
        };
        let visible = self.visible_columns(viewport_width);
        let row_count = self.rows.len();
        div()
            .id("results-scroll")
            .size_full()
            .overflow_x_scroll()
            // Without this, gpui's cross-axis fallback turns every vertical
            // wheel tick into horizontal scroll here (this container has no
            // vertical overflow), so the grid slides sideways instead of down.
            .restrict_scroll_to_axis()
            .track_scroll(&self.column_scroll)
            // The visible column range is derived from this offset, and scroll
            // itself does not re-render the view. Notify only for horizontal
            // input, so wheel/trackpad panning does not rebuild rows.
            .on_scroll_wheel(cx.listener(|_, event: &ScrollWheelEvent, _, cx| {
                let horizontal = match event.delta {
                    ScrollDelta::Pixels(delta) => delta.x != px(0.),
                    ScrollDelta::Lines(delta) => delta.x != 0.0,
                };
                if horizontal {
                    cx.notify();
                }
            }))
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
                    .child(self.render_header(visible.clone()))
                    .child(div().flex_1().min_h_0().child({
                        let mut rows = uniform_list(
                            "result-rows",
                            row_count,
                            cx.processor(
                                move |this, range: std::ops::Range<usize>, _window, _cx| {
                                    this.last_rendered = range.clone();
                                    range
                                        .map(|ix| this.render_row(ix, visible.clone()))
                                        .collect::<Vec<_>>()
                                },
                            ),
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

    #[test]
    fn visible_columns_follow_the_scroll_handle_offset() {
        let mut g = ResultGrid::new();
        g.set_columns((0..100).map(|i| col(&format!("c{i}"))).collect());
        // gpui keeps a scrolled-right offset negative (clamped to
        // -max_offset..=0), so the grid must flip the sign before treating it
        // as a distance. Passing the raw offset pinned the window to the first
        // columns and left the rest of a wide table blank.
        g.column_scroll.set_offset(gpui::point(px(-900.), px(0.)));
        assert_eq!(g.visible_columns(800.0), 3..12);
        // An unscrolled grid still starts at the first column.
        g.column_scroll.set_offset(gpui::point(px(0.), px(0.)));
        assert_eq!(g.visible_columns(800.0), 0..7);
    }

    #[test]
    fn visible_columns_follow_the_horizontal_offset() {
        // 100 columns × 180 px; an 800 px viewport shows five plus overscan.
        assert_eq!(visible_column_range(100, 0.0, 800.0), 0..7);
        // Offset 900 starts inside column 5; overscan reaches back to 3.
        assert_eq!(visible_column_range(100, 900.0, 800.0), 3..12);
        // Near the end, the range clamps to the column count.
        assert_eq!(visible_column_range(10, 1620.0, 800.0), 7..10);
        assert_eq!(visible_column_range(0, 0.0, 800.0), 0..0);
        // A missing layout pass (width 0) still renders at least one column.
        assert_eq!(visible_column_range(4, 0.0, 0.0), 0..3);
    }
}
