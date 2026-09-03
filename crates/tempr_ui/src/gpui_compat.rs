//! Compatibility shim over `gpui` + `gpui_platform` + `gpui_tokio`.
//!
//! Application bootstrap, window opening and the tokio bridge are wrapped here
//! so that when upstream renames a method or moves a type between crates, the
//! fix is isolated to this file (docs/11-gpui.md → "GPUI compatibility shim").

use gpui::{App, AppContext, Bounds, Entity, Render, WindowBounds, WindowOptions, px, size};
use tokio::task::JoinError;

/// Default main-window size in logical pixels.
pub const DEFAULT_WINDOW_SIZE: (f32, f32) = (1200.0, 800.0);

/// Run the GPUI application. `init` runs once on the main thread with the
/// tokio bridge already installed; open windows from inside it.
pub fn run_app(init: impl FnOnce(&mut App) + 'static) {
    gpui_platform::application().run(move |cx: &mut App| {
        gpui_tokio::init(cx);
        init(cx);
    });
}

/// Open a centered top-level window whose root entity is built by `build`.
pub fn open_main_window<V: Render>(
    cx: &mut App,
    title: &str,
    build: impl FnOnce(&mut App) -> V + 'static,
) -> anyhow::Result<Entity<V>> {
    let (w, h) = DEFAULT_WINDOW_SIZE;
    let bounds = Bounds::centered(None, size(px(w), px(h)), cx);
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        titlebar: Some(gpui::TitlebarOptions {
            title: Some(title.to_owned().into()),
            ..Default::default()
        }),
        ..Default::default()
    };
    let mut root: Option<Entity<V>> = None;
    cx.open_window(options, |_window, cx| {
        let entity = cx.new(|cx| build(cx));
        root = Some(entity.clone());
        entity
    })?;
    cx.activate(true);
    root.ok_or_else(|| anyhow::anyhow!("open_window returned without building a root view"))
}

/// Run a tokio future on the shared runtime, returning a GPUI task.
/// Dropping the task cancels the work — call `.detach()` to fire-and-forget.
pub fn spawn_tokio<F>(cx: &App, fut: F) -> gpui::Task<Result<F::Output, JoinError>>
where
    F: std::future::Future + Send + 'static,
    F::Output: Send + 'static,
{
    gpui_tokio::Tokio::spawn(cx, fut)
}
