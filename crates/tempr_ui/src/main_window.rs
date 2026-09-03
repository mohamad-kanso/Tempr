//! Root view of the application window.
//!
//! Phase 1 shell: a vertical layout with a placeholder editor area (top) and a
//! placeholder results area (bottom). Real `Input` and `Table` components land
//! in follow-up tasks — see docs/PROGRESS.md.

use gpui::{Context, IntoElement, Render, Window, div, prelude::*, px, rgb};

/// Root entity of the main window. Holds only service handles and render
/// snapshots — never business logic (D6).
pub struct MainWindow {
    workspace_name: String,
}

impl MainWindow {
    pub fn new(workspace_name: impl Into<String>) -> Self {
        Self {
            workspace_name: workspace_name.into(),
        }
    }
}

impl Render for MainWindow {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(rgb(0x1e1e2e))
            .text_color(rgb(0xcdd6f4))
            .child(
                div()
                    .flex()
                    .items_center()
                    .h(px(32.0))
                    .px_3()
                    .bg(rgb(0x181825))
                    .child(format!("Tempr — {}", self.workspace_name)),
            )
            .child(
                div()
                    .flex_1()
                    .p_3()
                    .border_b_1()
                    .border_color(rgb(0x313244))
                    .child("-- SQL editor (Phase 1 placeholder)"),
            )
            .child(
                div()
                    .flex_1()
                    .p_3()
                    .child("Results grid (Phase 1 placeholder)"),
            )
    }
}
