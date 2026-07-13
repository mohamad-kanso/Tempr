use crate::connection::Connection;
use crate::history::HistoryEntry;
use crate::ids::WorkspaceId;
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct Workspace {
    pub id: WorkspaceId,
    pub name: String,
    pub root_path: PathBuf,
    pub connections: Vec<Connection>,
    pub history: Vec<HistoryEntry>,
    pub settings: WorkspaceSettings,
}

#[derive(Debug, Clone)]
pub struct SqlFile {
    pub id: crate::ids::SqlFileId,
    pub name: String,
    pub relative_path: PathBuf,
    pub content_hash: [u8; 32],
}

#[derive(Debug, Clone)]
pub struct WorkspaceSettings {
    pub autosave_interval_ms: u64,
    pub max_result_rows: usize,
}

impl Default for WorkspaceSettings {
    fn default() -> Self {
        Self {
            autosave_interval_ms: 500,
            max_result_rows: 10_000,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::WorkspaceId;

    #[test]
    fn workspace_settings_defaults() {
        let s = WorkspaceSettings::default();
        assert_eq!(s.autosave_interval_ms, 500);
        assert_eq!(s.max_result_rows, 10_000);
    }

    #[test]
    fn workspace_construction() {
        let ws = Workspace {
            id: WorkspaceId::new(),
            name: "test-project".to_string(),
            root_path: PathBuf::from("/tmp/test-project"),
            connections: vec![],
            history: vec![],
            settings: WorkspaceSettings::default(),
        };
        assert_eq!(ws.name, "test-project");
        assert!(ws.connections.is_empty());
    }
}
