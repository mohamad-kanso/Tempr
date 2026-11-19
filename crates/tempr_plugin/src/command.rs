//! The one GPUI action every plugin command travels through.
//!
//! GPUI dispatches by action *type*, and the host cannot name the action
//! types of a crate it does not depend on. Binding a payload action whose
//! payload is the command id solves both the keymap and the palette with one
//! handler (docs/superpowers/specs/2026-11-18-phase4-… §7.4).
//!
//! `no_json` is what keeps `serde` and `schemars` out of the plugin surface:
//! the derive needs them to build actions from JSON keymaps, and Tempr
//! installs every binding in code.

use gpui::{Action, SharedString};

#[derive(Clone, PartialEq, Action)]
#[action(namespace = plugin, no_json)]
pub struct PluginCommand {
    /// Command id, as registered by the owning plugin.
    pub id: SharedString,
}

impl PluginCommand {
    pub fn new(id: impl Into<SharedString>) -> Self {
        Self { id: id.into() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_action_carries_its_command_id() {
        let action = PluginCommand::new("dev.tempr.demo::run");
        assert_eq!(action.id.as_ref(), "dev.tempr.demo::run");
        assert_eq!(action.name(), "plugin::PluginCommand");
    }

    #[test]
    fn cloning_preserves_the_payload() {
        let action = PluginCommand::new("dev.tempr.demo::run");
        let clone = action.boxed_clone();
        let boxed: Box<dyn std::any::Any> = clone;
        let downcast = boxed.downcast::<PluginCommand>().expect("same type");
        assert_eq!(downcast.id.as_ref(), "dev.tempr.demo::run");
    }
}
