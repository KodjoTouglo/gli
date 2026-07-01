//! Filesystem abstraction so modules work locally or over a remote transport.
//!
//! Modules never call `tokio::fs` directly; they go through [`Context`] methods
//! backed by a [`FileSystem`]. The local implementation uses the real disk; a
//! remote (SFTP) implementation can be swapped in for agentless execution.

use std::path::Path;

use async_trait::async_trait;

use crate::{Error, Result};

/// Minimal filesystem operations the modules need. Paths are already resolved
/// against the context root by the caller.
#[async_trait]
pub trait FileSystem: Send + Sync {
    /// Read a file, returning `None` when it does not exist.
    async fn read(&self, path: &Path) -> Result<Option<String>>;

    /// Write a file, creating parent directories as needed.
    async fn write(&self, path: &Path, body: &str) -> Result<()>;

    /// Remove a file; succeeds if it is already absent.
    async fn remove(&self, path: &Path) -> Result<()>;

    /// Whether the path exists.
    async fn exists(&self, path: &Path) -> Result<bool>;

    /// Rename (move) a file.
    async fn rename(&self, from: &Path, to: &Path) -> Result<()>;
}

/// Local disk implementation backed by `tokio::fs`.
#[derive(Debug, Default)]
pub struct LocalFs;

#[async_trait]
impl FileSystem for LocalFs {
    async fn read(&self, path: &Path) -> Result<Option<String>> {
        match tokio::fs::read_to_string(path).await {
            Ok(c) => Ok(Some(c)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(Error::io(path.display().to_string(), e)),
        }
    }

    async fn write(&self, path: &Path, body: &str) -> Result<()> {
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| Error::io(parent.display().to_string(), e))?;
        }
        tokio::fs::write(path, body)
            .await
            .map_err(|e| Error::io(path.display().to_string(), e))
    }

    async fn remove(&self, path: &Path) -> Result<()> {
        match tokio::fs::remove_file(path).await {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(Error::io(path.display().to_string(), e)),
        }
    }

    async fn exists(&self, path: &Path) -> Result<bool> {
        Ok(tokio::fs::try_exists(path).await.unwrap_or(false))
    }

    async fn rename(&self, from: &Path, to: &Path) -> Result<()> {
        tokio::fs::rename(from, to)
            .await
            .map_err(|e| Error::io(to.display().to_string(), e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn write_creates_dirs_read_round_trips_remove_is_idempotent() {
        let tmp = tempfile::tempdir().unwrap();
        let fs = LocalFs;
        let path = tmp.path().join("a/b/c.txt");

        assert_eq!(fs.read(&path).await.unwrap(), None);
        fs.write(&path, "hi").await.unwrap();
        assert_eq!(fs.read(&path).await.unwrap().as_deref(), Some("hi"));
        assert!(fs.exists(&path).await.unwrap());

        fs.remove(&path).await.unwrap();
        assert!(!fs.exists(&path).await.unwrap());
        // Removing a missing file is a no-op, not an error.
        fs.remove(&path).await.unwrap();
    }

    #[tokio::test]
    async fn rename_moves_file() {
        let tmp = tempfile::tempdir().unwrap();
        let fs = LocalFs;
        let from = tmp.path().join("from.txt");
        let to = tmp.path().join("to.txt");
        fs.write(&from, "x").await.unwrap();
        fs.rename(&from, &to).await.unwrap();
        assert_eq!(fs.read(&to).await.unwrap().as_deref(), Some("x"));
        assert_eq!(fs.read(&from).await.unwrap(), None);
    }
}
