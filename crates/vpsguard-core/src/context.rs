//! Execution context handed to every module call.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::{CommandRunner, Config, DistroFamily, FileSystem, LocalFs, Platform, SystemRunner};

/// Shared, read-only context for a module operation.
///
/// `root` is the filesystem root the module operates against. In production it
/// is `/`; tests point it at a tempdir. Filesystem and command access go through
/// injected backends so the same modules run locally or over a remote transport.
#[derive(Clone)]
pub struct Context {
    pub config: Config,
    root: PathBuf,
    runner: Arc<dyn CommandRunner>,
    fs: Arc<dyn FileSystem>,
    platform: Platform,
}

impl Context {
    /// Production context: real root, system runner, local fs, detected platform.
    pub fn system(config: Config) -> Self {
        Self {
            config,
            root: PathBuf::from("/"),
            runner: Arc::new(SystemRunner),
            fs: Arc::new(LocalFs),
            platform: Platform::detect(),
        }
    }

    /// Build a context with explicit root and runner (used by tests). Uses the
    /// local filesystem and defaults the platform to Debian.
    pub fn with_parts(config: Config, root: PathBuf, runner: Arc<dyn CommandRunner>) -> Self {
        Self {
            config,
            root,
            runner,
            fs: Arc::new(LocalFs),
            platform: Platform::of(DistroFamily::Debian),
        }
    }

    /// Override the platform (tests exercising distro-specific behaviour).
    pub fn with_platform(mut self, platform: Platform) -> Self {
        self.platform = platform;
        self
    }

    /// Override the filesystem backend (remote execution).
    pub fn with_fs(mut self, fs: Arc<dyn FileSystem>) -> Self {
        self.fs = fs;
        self
    }

    /// Resolve an absolute-looking path (`/etc/...`) against [`Self::root`].
    pub fn path(&self, absolute: impl AsRef<Path>) -> PathBuf {
        let p = absolute.as_ref();
        let rel = p.strip_prefix("/").unwrap_or(p);
        self.root.join(rel)
    }

    pub fn runner(&self) -> &dyn CommandRunner {
        self.runner.as_ref()
    }

    pub fn platform(&self) -> &Platform {
        &self.platform
    }

    // Filesystem helpers (root-resolved), delegating to the backend.

    /// Read a file, `None` if absent.
    pub async fn read(&self, absolute: impl AsRef<Path>) -> crate::Result<Option<String>> {
        self.fs.read(&self.path(absolute)).await
    }

    /// Read a file, empty string if absent.
    pub async fn read_or_empty(&self, absolute: impl AsRef<Path>) -> crate::Result<String> {
        Ok(self.read(absolute).await?.unwrap_or_default())
    }

    /// Write a file, creating parent directories.
    pub async fn write(&self, absolute: impl AsRef<Path>, body: &str) -> crate::Result<()> {
        self.fs.write(&self.path(absolute), body).await
    }

    /// Remove a file; ok if absent.
    pub async fn remove(&self, absolute: impl AsRef<Path>) -> crate::Result<()> {
        self.fs.remove(&self.path(absolute)).await
    }

    /// Whether a path exists.
    pub async fn exists(&self, absolute: impl AsRef<Path>) -> crate::Result<bool> {
        self.fs.exists(&self.path(absolute)).await
    }

    /// Rename a file (both paths root-resolved).
    pub async fn rename(&self, from: impl AsRef<Path>, to: impl AsRef<Path>) -> crate::Result<()> {
        self.fs.rename(&self.path(from), &self.path(to)).await
    }
}
