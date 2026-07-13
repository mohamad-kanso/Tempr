use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const CURRENT_FORMAT_VERSION: u32 = 1;

/// On-disk representation of workspace.toml.
/// This is a serialisation type — not the same as the domain `Workspace` struct.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceManifest {
    pub workspace: WorkspaceInfo,
    #[serde(default)]
    pub connections: Vec<ConnectionConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceInfo {
    pub name: String,
    pub format_version: u32,
}

/// Persisted connection definition — stores config and keychain reference, never secrets.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectionConfig {
    pub id: Uuid,
    pub name: String,
    pub driver: String,
    pub host: String,
    pub port: u16,
    pub database: String,
    pub username: String,
    /// Opaque reference resolved by the OS keychain at runtime — never a raw password.
    pub secret_ref: String,
}

impl WorkspaceManifest {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            workspace: WorkspaceInfo {
                name: name.into(),
                format_version: CURRENT_FORMAT_VERSION,
            },
            connections: vec![],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_toml_roundtrip_empty() {
        let manifest = WorkspaceManifest::new("test-project");
        let toml_str = toml::to_string(&manifest).expect("serialize");
        let back: WorkspaceManifest = toml::from_str(&toml_str).expect("deserialize");
        assert_eq!(back.workspace.name, "test-project");
        assert_eq!(back.workspace.format_version, CURRENT_FORMAT_VERSION);
        assert!(back.connections.is_empty());
    }

    #[test]
    fn manifest_toml_roundtrip_with_connections() {
        let mut manifest = WorkspaceManifest::new("my-project");
        manifest.connections.push(ConnectionConfig {
            id: Uuid::new_v4(),
            name: "prod-db".to_string(),
            driver: "postgres".to_string(),
            host: "db.example.com".to_string(),
            port: 5432,
            database: "production".to_string(),
            username: "admin".to_string(),
            secret_ref: "keychain://tempr/prod-db".to_string(),
        });

        let toml_str = toml::to_string(&manifest).expect("serialize");
        let back: WorkspaceManifest = toml::from_str(&toml_str).expect("deserialize");
        assert_eq!(back.connections.len(), 1);
        assert_eq!(back.connections[0].name, "prod-db");
        assert_eq!(back.connections[0].port, 5432);
    }

    #[test]
    fn malformed_toml_returns_error() {
        let bad = "workspace = !!!invalid toml!!!";
        let result: Result<WorkspaceManifest, _> = toml::from_str(bad);
        assert!(result.is_err(), "malformed TOML must error");
    }

    #[test]
    fn missing_required_field_returns_error() {
        let bad = r#"
[workspace]
format_version = 1
# name is missing
"#;
        let result: Result<WorkspaceManifest, _> = toml::from_str(bad);
        assert!(result.is_err(), "missing required field must error");
    }
}
