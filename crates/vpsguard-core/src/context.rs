//! Execution context handed to every module call.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::{CommandRunner, Config, DistroFamily, Platform, SystemRunner};

/// Shared, read-only context for a module operation.
///
/// `root` is the filesystem root the module operates against. In production it
/// is `/`; tests point it at a tempdir so `apply` can rewrite a throwaway
/// `etc/ssh/sshd_config` without touching the host.
#[derive(Clone)]
pub struct Context {
    pub config: Config,
    root: PathBuf,
    runner: Arc<dyn CommandRunner>,
    platform: Platform,
}

impl Context {
    /// Production context: real root, system runner, detected platform.
    pub fn system(config: Config) -> Self {
        Self {
            config,
            root: PathBuf::from("/"),
            runner: Arc::new(SystemRunner),
            platform: Platform::detect(),
        }
    }

    /// Build a context with explicit root and runner (used by tests). Platform
    /// defaults to Debian (the primary target); override with `with_platform`.
    pub fn with_parts(config: Config, root: PathBuf, runner: Arc<dyn CommandRunner>) -> Self {
        Self {
            config,
            root,
            runner,
            platform: Platform::of(DistroFamily::Debian),
        }
    }

    /// Override the platform (tests exercising distro-specific behaviour).
    pub fn with_platform(mut self, platform: Platform) -> Self {
        self.platform = platform;
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
}
