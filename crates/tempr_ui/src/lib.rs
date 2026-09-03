//! `tempr_ui` — GPUI components, views and layout.
//!
//! Rules (docs/11-gpui.md, D6, D16):
//! - No business logic here. Views hold service handles + immutable snapshots.
//! - Every GPUI / `gpui_platform` bootstrap call goes through [`gpui_compat`]
//!   so upstream churn is isolated to one module.
//! - Only Apache-2.0 Zed crates (`gpui`, `gpui_platform`, `gpui_tokio`).

pub mod gpui_compat;
pub mod main_window;

pub use main_window::MainWindow;
