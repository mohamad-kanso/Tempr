//! Command identity (docs/05-services.md → CommandService).

use serde::{Deserialize, Serialize};

/// Identifier of a user action, namespaced like a GPUI action name
/// (`main_window::RunQuery`, `plugin_id::command`). Stable across sessions;
/// used as the key in keybinding configuration.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CommandId(pub String);

impl CommandId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for CommandId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<&str> for CommandId {
    fn from(s: &str) -> Self {
        Self(s.to_string())
    }
}
