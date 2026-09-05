//! The typed command catalog: every user action in the UI as a GPUI action
//! type plus its palette metadata (docs/05-services.md → CommandService,
//! D23). `CommandService` holds the metadata and resolved keybindings; this
//! module knows how to turn a command id back into a concrete GPUI action
//! (for dispatch) and a `KeyBinding` (for `cx.bind_keys`).
//!
//! Adding an action = adding one `spec::<A>()` line here. There is no other
//! keybinding table anywhere: this catalog *is* the keyboard-only audit.

use std::sync::Arc;

use gpui::{Action, App, KeyBinding, Keystroke, Window};
use tempr_domain::CommandId;
use tempr_services::{CommandContribution, CommandService};

use crate::components::input;
use crate::components::palette;
use crate::main_window;

/// One catalog entry: metadata plus constructors for the typed action.
pub struct CommandSpec {
    pub id: CommandId,
    pub title: &'static str,
    pub category: &'static str,
    pub context: Option<&'static str>,
    pub default_keystrokes: &'static [&'static str],
    make_action: fn() -> Box<dyn Action>,
    make_binding: fn(&str, Option<&str>) -> KeyBinding,
}

impl CommandSpec {
    pub fn action(&self) -> Box<dyn Action> {
        (self.make_action)()
    }

    /// A binding for this command, or `None` if `keystrokes` does not parse
    /// (user-provided overrides must never panic).
    pub fn binding(&self, keystrokes: &str) -> Option<KeyBinding> {
        let valid = !keystrokes.trim().is_empty()
            && keystrokes
                .split_whitespace()
                .all(|k| Keystroke::parse(k).is_ok());
        valid.then(|| (self.make_binding)(keystrokes, self.context))
    }

    pub fn contribution(&self) -> CommandContribution {
        CommandContribution {
            id: self.id.clone(),
            title: self.title.to_string(),
            category: self.category.to_string(),
            context: self.context.map(str::to_string),
            default_keystrokes: self
                .default_keystrokes
                .iter()
                .map(|k| k.to_string())
                .collect(),
        }
    }
}

fn spec<A: Action + Default>(
    title: &'static str,
    category: &'static str,
    context: Option<&'static str>,
    default_keystrokes: &'static [&'static str],
) -> CommandSpec {
    CommandSpec {
        id: CommandId::new(A::default().name()),
        title,
        category,
        context,
        default_keystrokes,
        make_action: || Box::new(A::default()),
        make_binding: |keys, ctx| KeyBinding::new(keys, A::default(), ctx),
    }
}

const MW: Option<&str> = Some(main_window::KEY_CONTEXT);
const IN: Option<&str> = Some(input::KEY_CONTEXT);
const PAL: Option<&str> = Some(palette::KEY_CONTEXT);

/// Every core command. Order here is only the fallback for the palette
/// (it sorts by category/title anyway).
pub fn core_commands() -> Vec<CommandSpec> {
    use main_window as mw;
    vec![
        // Query
        spec::<mw::RunQuery>("Run Query", "Query", MW, &["ctrl-enter", "cmd-enter"]),
        spec::<mw::CancelQuery>("Cancel Query", "Query", MW, &["escape"]),
        // View
        spec::<mw::TogglePalette>(
            "Toggle Command Palette",
            "View",
            None,
            &["ctrl-shift-p", "cmd-shift-p"],
        ),
        spec::<mw::DebugScrollBenchmark>(
            "Debug: Scroll Benchmark",
            "View",
            MW,
            &["ctrl-shift-b", "cmd-shift-b"],
        ),
        // Application
        spec::<mw::Quit>("Quit", "Application", None, &["ctrl-q", "cmd-q"]),
        // Palette (only while it is open)
        spec::<palette::SelectNext>("Palette: Next Item", "Palette", PAL, &["down", "ctrl-n"]),
        spec::<palette::SelectPrev>("Palette: Previous Item", "Palette", PAL, &["up", "ctrl-p"]),
        spec::<palette::Dismiss>("Palette: Close", "Palette", PAL, &["escape"]),
        // Edit (single-line Input)
        spec::<input::Backspace>("Edit: Backspace", "Edit", IN, &["backspace"]),
        spec::<input::Delete>("Edit: Delete", "Edit", IN, &["delete"]),
        spec::<input::Left>("Edit: Move Left", "Edit", IN, &["left"]),
        spec::<input::Right>("Edit: Move Right", "Edit", IN, &["right"]),
        spec::<input::SelectLeft>("Edit: Select Left", "Edit", IN, &["shift-left"]),
        spec::<input::SelectRight>("Edit: Select Right", "Edit", IN, &["shift-right"]),
        spec::<input::SelectAll>("Edit: Select All", "Edit", IN, &["ctrl-a", "cmd-a"]),
        spec::<input::Home>("Edit: Line Start", "Edit", IN, &["home"]),
        spec::<input::End>("Edit: Line End", "Edit", IN, &["end"]),
        spec::<input::Paste>("Edit: Paste", "Edit", IN, &["ctrl-v", "cmd-v"]),
        spec::<input::Cut>("Edit: Cut", "Edit", IN, &["ctrl-x", "cmd-x"]),
        spec::<input::Copy>("Edit: Copy", "Edit", IN, &["ctrl-c", "cmd-c"]),
        spec::<input::Submit>("Edit: Submit", "Edit", IN, &["enter"]),
    ]
}

/// Register the catalog with the service and bind the *resolved* keystrokes
/// (defaults overridden by user/workspace layers already set on the
/// service). Call once at startup, after the service has its layers.
pub fn install(cx: &mut App, service: &CommandService) {
    let specs = core_commands();
    for s in &specs {
        service.register(s.contribution());
    }
    let mut bindings = Vec::new();
    for s in &specs {
        for keys in service.keystrokes_for(&s.id) {
            match s.binding(&keys) {
                Some(b) => bindings.push(b),
                None => tracing::warn!(command = %s.id, keys, "invalid keystroke ignored"),
            }
        }
    }
    cx.bind_keys(bindings);
}

/// Look a command up in the catalog.
pub fn find(id: &CommandId) -> Option<CommandSpec> {
    core_commands().into_iter().find(|s| &s.id == id)
}

/// Dispatch `id` as a GPUI action into `window` (as if its keybinding had
/// been pressed) and record the execution on the service. Returns `false`
/// for an unknown id.
pub fn dispatch(
    id: &CommandId,
    service: &Arc<CommandService>,
    window: &mut Window,
    cx: &mut App,
) -> bool {
    let Some(spec) = find(id) else {
        return false;
    };
    window.dispatch_action(spec.action(), cx);
    service.record_executed(id.clone());
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_gpui_action_names_and_unique() {
        let specs = core_commands();
        let mut ids: Vec<&str> = specs.iter().map(|s| s.id.as_str()).collect();
        assert!(ids.contains(&"main_window::RunQuery"));
        assert!(ids.contains(&"input::Submit"));
        assert!(ids.contains(&"palette::Dismiss"));
        let n = ids.len();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), n, "duplicate command ids");
    }

    #[test]
    fn every_command_has_a_default_keystroke() {
        for s in core_commands() {
            assert!(
                !s.default_keystrokes.is_empty(),
                "{} has no default keybinding (no mouse-only features)",
                s.id
            );
        }
    }

    #[test]
    fn actions_round_trip_to_their_ids() {
        for s in core_commands() {
            assert_eq!(s.action().name(), s.id.as_str());
        }
    }

    #[test]
    fn invalid_keystrokes_are_rejected_without_panicking() {
        let run = find(&CommandId::from("main_window::RunQuery")).unwrap();
        assert!(run.binding("ctrl-enter").is_some());
        assert!(run.binding("ctrl-k ctrl-r").is_some(), "chords");
        assert!(run.binding("").is_none());
        assert!(run.binding("   ").is_none());
    }
}
