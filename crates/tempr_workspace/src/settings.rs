//! User-level settings (`~/.config/tempr/settings.toml`) — the middle layer of
//! the three-layer settings model in docs/04-workspace.md. Workspace-level
//! overrides live in `workspace.toml` (`WorkspaceManifest::keybindings`).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::WorkspaceError;

/// `command id → keystrokes` (GPUI syntax, e.g. `"ctrl-enter"`). An empty
/// list unbinds the command.
pub type KeybindingMap = BTreeMap<String, Vec<String>>;

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct UserSettings {
    #[serde(default)]
    pub keybindings: KeybindingMap,
}

/// `~/.config/tempr/settings.toml` (platform config dir), if resolvable.
pub fn user_settings_path() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("tempr").join("settings.toml"))
}

/// Parse the TOML text of a settings file.
pub fn parse_user_settings(text: &str) -> Result<UserSettings, WorkspaceError> {
    toml::from_str(text).map_err(|e| WorkspaceError::Corrupted {
        reason: format!("settings.toml: {e}"),
    })
}

/// Load settings from `path`; a missing file is not an error (defaults).
pub fn load_user_settings_from(path: &Path) -> Result<UserSettings, WorkspaceError> {
    match std::fs::read_to_string(path) {
        Ok(text) => parse_user_settings(&text),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(UserSettings::default()),
        Err(e) => Err(WorkspaceError::Io(e)),
    }
}

/// Load the user's settings from the platform config dir.
pub fn load_user_settings() -> Result<UserSettings, WorkspaceError> {
    match user_settings_path() {
        Some(p) => load_user_settings_from(&p),
        None => Ok(UserSettings::default()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_keybindings_table() {
        let s = parse_user_settings(
            r#"
[keybindings]
"main_window::RunQuery" = ["f5", "ctrl-enter"]
"main_window::Quit" = []
"#,
        )
        .unwrap();
        assert_eq!(
            s.keybindings["main_window::RunQuery"],
            vec!["f5", "ctrl-enter"]
        );
        assert!(s.keybindings["main_window::Quit"].is_empty());
    }

    #[test]
    fn empty_and_missing_are_defaults() {
        assert_eq!(parse_user_settings("").unwrap(), UserSettings::default());
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("nope.toml");
        assert_eq!(
            load_user_settings_from(&missing).unwrap(),
            UserSettings::default()
        );
    }

    #[test]
    fn malformed_is_an_error_not_a_panic() {
        assert!(matches!(
            parse_user_settings("keybindings = 3"),
            Err(WorkspaceError::Corrupted { .. })
        ));
    }
}
