//! Small IO helpers shared by modules.

use std::path::{Path, PathBuf};

use vpsguard_core::{Error, Result};

/// Write `body` to `path`, creating parent directories as needed.
pub(crate) async fn write(path: &Path, body: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| Error::io(parent.display().to_string(), e))?;
    }
    tokio::fs::write(path, body)
        .await
        .map_err(|e| Error::io(path.display().to_string(), e))
}

/// Append `suffix` to a path's filename (e.g. `.bak`).
pub(crate) fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut s = path.as_os_str().to_os_string();
    s.push(suffix);
    PathBuf::from(s)
}
