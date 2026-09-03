//! `tempr_ui` — GPUI components, views and layout.
//!
//! Rules (docs/11-gpui.md, D6, D16/D17):
//! - No business logic here. Views hold service handles + render state.
//! - Every GPUI / `gpui_platform` / `gpui_tokio` bootstrap call goes through
//!   [`gpui_compat`] so upstream churn is isolated to one module.
//! - Only Apache-2.0 Zed crates (`gpui`, `gpui_platform`, `gpui_tokio`).

pub mod components;
pub mod events;
pub mod gpui_compat;
pub mod main_window;
pub mod scroll_bench;
pub mod theme;
pub mod value_format;

pub use main_window::{DevOptions, MainWindow, Quit, Services, bind_keys};
