//! Root view of the application window.
//!
//! Phase 1 shell: a vertical layout with a placeholder editor area (top) and a
//! placeholder results area (bottom). Real `Input` and `Table` components land
//! in follow-up tasks — see docs/PROGRESS.md.

use gpui::{Context, IntoElement, Render, Window, div, prelude::*, px, rgb};

// Placeholder palette. Components must not hard-code colors (docs/11-gpui.md
// → Theming); these consts are the single seam to replace with `ThemeProvider`
// tokens once the theme system lands (docs/TODO.md).
const SURFACE: u32 = 0x1e1e2e;
const SURFACE_RAISED: u32 = 0x181825;
const BORDER: u32 = 0x313244;
const TEXT: u32 = 0xcdd6f4;

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
            .bg(rgb(SURFACE))
            .text_color(rgb(TEXT))
            .child(
                div()
                    .flex()
                    .items_center()
                    .h(px(32.0))
                    .px_3()
                    .bg(rgb(SURFACE_RAISED))
                    .child(format!("Tempr — {}", self.workspace_name)),
            )
            .child(
                div()
                    .flex_1()
                    .p_3()
                    .border_b_1()
                    .border_color(rgb(BORDER))
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
