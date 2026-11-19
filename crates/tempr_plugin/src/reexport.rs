//! The only window a plugin gets onto GPUI and the driver traits.
//!
//! Re-exporting instead of letting plugins depend on `gpui` directly keeps
//! the pinned rev and the `gpui_compat` shim (docs/11-gpui.md) out of every
//! plugin's manifest, and makes the API surface auditable in one place.

pub use gpui;
pub use gpui::{
    AnyElement, App, Bounds, Context, ElementId, Entity, FocusHandle, IntoElement, ParentElement,
    Pixels, Render, SharedString, Styled, Window, div, px, rgb,
};
pub use tempr_db::{
    DatabaseDriver, DriverConnection, DriverError, EngineId, QueryStream, SchemaScope,
};
