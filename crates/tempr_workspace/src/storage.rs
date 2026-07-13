use crate::error::WorkspaceError;
use crate::manifest::WorkspaceManifest;
use async_trait::async_trait;
use std::path::PathBuf;

/// Gateway for all workspace file-system access.
/// No module performs raw std::fs calls outside this trait.
#[async_trait]
pub trait Storage: Send + Sync {
    /// Load and parse the workspace manifest from `workspace.toml`.
    async fn load_manifest(&self) -> Result<WorkspaceManifest, WorkspaceError>;

    /// Persist the manifest using an atomic write (temp file → rename).
    async fn save_manifest(&self, manifest: &WorkspaceManifest) -> Result<(), WorkspaceError>;

    /// Create the workspace directory structure (idempotent).
    async fn init_workspace_dir(&self) -> Result<(), WorkspaceError>;

    /// Returns the path to the `.tempr/` subdirectory for derived state.
    fn tempr_dir(&self) -> PathBuf;

    /// Returns the platform-specific application data directory for global Tempr state.
    /// On Linux: ~/.local/share/tempr, macOS: ~/Library/Application Support/tempr,
    /// Windows: %APPDATA%/tempr
    fn app_data_dir() -> PathBuf
    where
        Self: Sized,
    {
        dirs::data_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("tempr")
    }
}

/// File-system backed `Storage` implementation.
pub struct FileSystemStorage {
    workspace_path: PathBuf,
}

impl FileSystemStorage {
    pub fn new(workspace_path: impl Into<PathBuf>) -> Self {
        Self {
            workspace_path: workspace_path.into(),
        }
    }

    fn manifest_path(&self) -> PathBuf {
        self.workspace_path.join("workspace.toml")
    }

    fn manifest_tmp_path(&self) -> PathBuf {
        self.workspace_path.join("workspace.toml.tmp")
    }
}

#[async_trait]
impl Storage for FileSystemStorage {
    async fn load_manifest(&self) -> Result<WorkspaceManifest, WorkspaceError> {
        let path = self.manifest_path();
        let bytes = tokio::fs::read(&path).await.map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                WorkspaceError::NotFound {
                    path: path.display().to_string(),
                }
            } else {
                WorkspaceError::Io(e)
            }
        })?;
        let text = String::from_utf8(bytes).map_err(|e| WorkspaceError::Corrupted {
            reason: format!("workspace.toml is not valid UTF-8: {e}"),
        })?;
        toml::from_str(&text).map_err(|e| WorkspaceError::Corrupted {
            reason: format!("workspace.toml parse error: {e}"),
        })
    }

    async fn save_manifest(&self, manifest: &WorkspaceManifest) -> Result<(), WorkspaceError> {
        let text = toml::to_string_pretty(manifest).map_err(|e| WorkspaceError::Corrupted {
            reason: format!("failed to serialise manifest: {e}"),
        })?;
        let tmp = self.manifest_tmp_path();
        let dst = self.manifest_path();

        // Atomic write: write to temp, then rename so a crash never corrupts the target.
        tokio::fs::write(&tmp, text.as_bytes()).await?;
        tokio::fs::rename(&tmp, &dst).await?;
        Ok(())
    }

    async fn init_workspace_dir(&self) -> Result<(), WorkspaceError> {
        tokio::fs::create_dir_all(&self.workspace_path).await?;
        tokio::fs::create_dir_all(self.tempr_dir()).await?;
        Ok(())
    }

    fn tempr_dir(&self) -> PathBuf {
        self.workspace_path.join(".tempr")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::{CURRENT_FORMAT_VERSION, ConnectionConfig, WorkspaceManifest};
    use tempfile::TempDir;
    use uuid::Uuid;

    async fn make_storage() -> (TempDir, FileSystemStorage) {
        let tmp = TempDir::new().expect("temp dir");
        let storage = FileSystemStorage::new(tmp.path());
        storage.init_workspace_dir().await.expect("init");
        (tmp, storage)
    }

    #[tokio::test]
    async fn save_and_load_roundtrip() {
        let (_dir, storage) = make_storage().await;
        let manifest = WorkspaceManifest::new("roundtrip-test");
        storage.save_manifest(&manifest).await.expect("save");

        let loaded = storage.load_manifest().await.expect("load");
        assert_eq!(loaded.workspace.name, "roundtrip-test");
        assert_eq!(loaded.workspace.format_version, CURRENT_FORMAT_VERSION);
    }

    #[tokio::test]
    async fn save_with_connections_roundtrip() {
        let (_dir, storage) = make_storage().await;
        let mut manifest = WorkspaceManifest::new("with-conn");
        manifest.connections.push(ConnectionConfig {
            id: Uuid::new_v4(),
            name: "local".to_string(),
            driver: "postgres".to_string(),
            host: "127.0.0.1".to_string(),
            port: 5432,
            database: "dev".to_string(),
            username: "postgres".to_string(),
            secret_ref: "keychain://tempr/local".to_string(),
        });
        storage.save_manifest(&manifest).await.expect("save");

        let loaded = storage.load_manifest().await.expect("load");
        assert_eq!(loaded.connections.len(), 1);
        assert_eq!(loaded.connections[0].host, "127.0.0.1");
    }

    #[tokio::test]
    async fn load_missing_file_returns_not_found() {
        let tmp = TempDir::new().expect("temp dir");
        let storage = FileSystemStorage::new(tmp.path());
        let err = storage.load_manifest().await.expect_err("should fail");
        assert!(matches!(err, WorkspaceError::NotFound { .. }));
    }

    #[tokio::test]
    async fn load_malformed_file_returns_corrupted() {
        let tmp = TempDir::new().expect("temp dir");
        let storage = FileSystemStorage::new(tmp.path());
        storage.init_workspace_dir().await.expect("init");

        let bad_path = tmp.path().join("workspace.toml");
        tokio::fs::write(&bad_path, b"workspace = !!!invalid!!!")
            .await
            .expect("write bad file");

        let err = storage.load_manifest().await.expect_err("should fail");
        assert!(matches!(err, WorkspaceError::Corrupted { .. }));
    }

    #[tokio::test]
    async fn init_creates_tempr_dir() {
        let tmp = TempDir::new().expect("temp dir");
        let storage = FileSystemStorage::new(tmp.path());
        storage.init_workspace_dir().await.expect("init");

        assert!(tmp.path().join(".tempr").is_dir());
    }

    #[tokio::test]
    async fn app_data_dir_is_absolute() {
        let dir = FileSystemStorage::app_data_dir();
        assert!(
            dir.is_absolute(),
            "app data dir must be absolute path: {:?}",
            dir
        );
        assert!(dir.ends_with("tempr"));
    }
}
