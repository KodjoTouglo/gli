//! Small path helpers shared by modules.

use std::path::{Path, PathBuf};

/// Append `suffix` to a path's filename (e.g. `.bak`).
pub(crate) fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut s = path.as_os_str().to_os_string();
    s.push(suffix);
    PathBuf::from(s)
}
