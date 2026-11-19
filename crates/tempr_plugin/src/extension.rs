//! The extension points a plugin can implement (docs/08-plugin-api.md).
//!
//! Each trait is the whole contract: the host stores instances in typed
//! vectors and calls them at a fixed point in its own pipeline. Nothing here
//! hands a plugin a service handle.

use std::path::PathBuf;
use std::sync::Arc;

use gpui::{AnyElement, App, FocusHandle, SharedString, Window};
use tempr_domain::{
    ColumnSpec, CommandId, CompletionItem, PluginCompletionRequest, QueryRun, Value,
};

/// Extra completion candidates, merged into the ranked pipeline.
///
/// Synchronous by contract: completion has a 5 ms budget for the whole
/// fan-out, and an `async` provider would let a plugin hide I/O behind it.
/// A provider only gets text, so its work is CPU-bound.
pub trait CompletionProvider: Send + Sync {
    /// Human-readable owner, used when logging a slow provider.
    fn provider_name(&self) -> &'static str;

    fn complete(&self, req: &PluginCompletionRequest<'_>) -> Vec<CompletionItem>;
}

/// Cell rendering for column types the built-in text formatter cannot serve
/// well. The host picks the first renderer whose `can_render` returns true,
/// in registration order, once per column.
pub trait ResultRenderer: Send + Sync {
    fn can_render(&self, col: &ColumnSpec) -> bool;

    /// Render one cell. `Value::Null` never reaches a renderer — the host
    /// keeps the null style on its own path.
    fn render_cell(&self, value: &Value, col: &ColumnSpec) -> AnyElement;
}

/// A panel in the workspace dock.
///
/// A trait rather than a closure because a panel owns state and needs
/// `&mut Window, &mut App` each frame, and because a plugin cannot create a
/// GPUI entity during `activate` (no window exists yet).
pub trait Panel: Send + Sync {
    /// Namespaced id, e.g. `dev.tempr.demo::stats`.
    fn panel_id(&self) -> &'static str;

    fn title(&self) -> SharedString;

    fn render(&mut self, window: &mut Window, cx: &mut App) -> AnyElement;

    /// Focus handle, when the panel wants focus for its own keybindings.
    fn focus_handle(&self, _cx: &App) -> Option<FocusHandle> {
        None
    }
}

/// Host-supplied data a plugin command may read. Read-only by construction:
/// the plugin never resolves a service, so the host decides what is visible.
#[derive(Default, Clone)]
pub struct CommandContext {
    /// The most recently completed run, if any.
    pub last_run: Option<Arc<QueryRun>>,
    /// Workspace directory, when one is open. Plugins may write derived files
    /// here (exports, reports); never credentials.
    pub workspace_root: Option<PathBuf>,
}

/// What a command did, for the host's status bar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandOutcome {
    Done(String),
    Failed(String),
}

impl CommandOutcome {
    pub fn done(message: impl Into<String>) -> Self {
        Self::Done(message.into())
    }

    pub fn failed(message: impl Into<String>) -> Self {
        Self::Failed(message.into())
    }

    pub fn is_failure(&self) -> bool {
        matches!(self, Self::Failed(_))
    }

    pub fn message(&self) -> &str {
        match self {
            Self::Done(m) | Self::Failed(m) => m,
        }
    }
}

type DispatchFn = dyn Fn(&mut Window, &mut App, &CommandContext) -> CommandOutcome + Send + Sync;

/// A command contributed by a plugin. It reaches the palette and the keymap
/// like a core command; the difference is dispatch (a closure, because the
/// host cannot name a plugin's action types).
pub struct PluginCommandSpec {
    /// Namespaced command id: `dev.tempr.export-csv::export_last_run`.
    pub id: CommandId,
    pub title: &'static str,
    pub category: &'static str,
    /// Keystrokes in GPUI syntax; empty for a palette-only command.
    pub default_keystrokes: Vec<&'static str>,
    pub dispatch: Arc<DispatchFn>,
}

impl PluginCommandSpec {
    pub fn new(
        id: CommandId,
        title: &'static str,
        category: &'static str,
        default_keystrokes: &[&'static str],
        dispatch: impl Fn(&mut Window, &mut App, &CommandContext) -> CommandOutcome
        + Send
        + Sync
        + 'static,
    ) -> Self {
        Self {
            id,
            title,
            category,
            default_keystrokes: default_keystrokes.to_vec(),
            dispatch: Arc::new(dispatch),
        }
    }
}
