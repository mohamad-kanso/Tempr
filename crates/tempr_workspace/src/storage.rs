use crate::error::WorkspaceError;
use crate::manifest::WorkspaceManifest;
use async_trait::async_trait;
use std::path::PathBuf;
use tempr_domain::{ConnectionId, SchemaSnapshot};

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

    /// Handle to this connection's catalog cache file. Creating the handle
    /// touches no disk; `load` and `save` do.
    fn catalog_cache(&self, connection: ConnectionId) -> Box<dyn CatalogCacheFile>;

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

/// Read/write access to one connection's `.tcat` cache. Every failure to read
/// is reported as `Ok(None)`: the cache is derived data and is rebuilt rather
/// than repaired.
#[async_trait]
pub trait CatalogCacheFile: Send + Sync {
    async fn load(&self) -> Result<Option<SchemaSnapshot>, WorkspaceError>;
    async fn save(&self, snapshot: &SchemaSnapshot) -> Result<(), WorkspaceError>;
    fn path(&self) -> PathBuf;
}

pub struct FileCatalogCache {
    path: PathBuf,
}

#[async_trait]
impl CatalogCacheFile for FileCatalogCache {
    async fn load(&self) -> Result<Option<SchemaSnapshot>, WorkspaceError> {
        let bytes = match tokio::fs::read(&self.path).await {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => {
                tracing::warn!(error = %e, path = %self.path.display(), "catalog cache unreadable");
                return Ok(None);
            }
        };
        crate::catalog::decode_catalog(&bytes)
    }

    async fn save(&self, snapshot: &SchemaSnapshot) -> Result<(), WorkspaceError> {
        let bytes = crate::catalog::encode_catalog(snapshot)?;
        if let Some(parent) = self.path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        let tmp = self.path.with_extension("tcat.tmp");
        tokio::fs::write(&tmp, &bytes).await?;
        tokio::fs::rename(&tmp, &self.path).await?;
        Ok(())
    }

    fn path(&self) -> PathBuf {
        self.path.clone()
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

    fn catalog_cache(&self, connection: ConnectionId) -> Box<dyn CatalogCacheFile> {
        Box::new(FileCatalogCache {
            path: self
                .tempr_dir()
                .join("cache")
                .join("catalog")
                .join(format!("{}.tcat", connection.0)),
        })
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
            tls: tempr_domain::TlsMode::Prefer,
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

    fn sample_snapshot(connection: tempr_domain::ConnectionId) -> tempr_domain::SchemaSnapshot {
        tempr_domain::SchemaSnapshot {
            id: tempr_domain::SchemaSnapshotId::new(),
            connection_id: connection,
            version: 1,
            fetched_at: chrono::Utc::now(),
            objects: vec![],
            keywords: vec!["select".to_string()],
            fingerprints: vec![],
        }
    }

    #[tokio::test]
    async fn catalog_cache_roundtrips_and_starts_empty() {
        let (_dir, storage) = make_storage().await;
        let connection = tempr_domain::ConnectionId::new();
        let cache = storage.catalog_cache(connection);

        assert!(cache.load().await.expect("load").is_none(), "no file yet");

        let snapshot = sample_snapshot(connection);
        cache.save(&snapshot).await.expect("save");
        let loaded = cache.load().await.expect("load").expect("file exists");
        assert_eq!(loaded.id, snapshot.id);
        assert_eq!(loaded.keywords, vec!["select".to_string()]);
    }

    #[tokio::test]
    async fn each_connection_gets_its_own_file() {
        let (_dir, storage) = make_storage().await;
        let a = tempr_domain::ConnectionId::new();
        let b = tempr_domain::ConnectionId::new();
        storage
            .catalog_cache(a)
            .save(&sample_snapshot(a))
            .await
            .expect("save a");

        assert!(
            storage
                .catalog_cache(b)
                .load()
                .await
                .expect("load b")
                .is_none()
        );
        assert_ne!(
            storage.catalog_cache(a).path(),
            storage.catalog_cache(b).path()
        );
    }

    #[tokio::test]
    async fn a_corrupt_cache_file_loads_as_none() {
        let (_dir, storage) = make_storage().await;
        let connection = tempr_domain::ConnectionId::new();
        let cache = storage.catalog_cache(connection);
        cache
            .save(&sample_snapshot(connection))
            .await
            .expect("save");

        tokio::fs::write(cache.path(), b"not a catalog file at all")
            .await
            .expect("corrupt the file");
        assert!(cache.load().await.expect("no error").is_none());
    }

    #[tokio::test]
    async fn saving_leaves_no_temp_file_behind() {
        let (_dir, storage) = make_storage().await;
        let connection = tempr_domain::ConnectionId::new();
        let cache = storage.catalog_cache(connection);
        cache
            .save(&sample_snapshot(connection))
            .await
            .expect("save");

        let dir = cache.path().parent().expect("parent").to_path_buf();
        let mut entries = tokio::fs::read_dir(&dir).await.expect("read dir");
        let mut names = Vec::new();
        while let Some(e) = entries.next_entry().await.expect("entry") {
            names.push(e.file_name().to_string_lossy().to_string());
        }
        assert!(
            names.iter().all(|n| !n.ends_with(".tmp")),
            "atomic write must not leave a temp file: {names:?}"
        );
    }
}
