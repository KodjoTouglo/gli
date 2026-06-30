//! The central `Module` trait every configuration unit implements.

use async_trait::async_trait;

use crate::{Change, Context, Report, Result, Status};

/// Grouping for ordering and UI. Security modules form the always-on baseline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Category {
    Security,
    System,
    Runtime,
    Network,
    App,
}

impl Category {
    /// Plan/dashboard ordering: security baseline first.
    pub fn rank(self) -> u8 {
        self as u8
    }
}

/// A unit of configuration: ssh, firewall, docker, ssl, app deploy.
///
/// Implementations must be idempotent: repeated `apply` converges to the same
/// state with no further side effects. `check` and `plan` never mutate.
#[async_trait]
pub trait Module: Send + Sync {
    /// Stable id used in config keys and the UI, e.g. `"ssh"`.
    fn name(&self) -> &str;

    /// One-line description shown in the plan and dashboard.
    fn summary(&self) -> &str;

    /// Category for ordering and grouping.
    fn category(&self) -> Category;

    /// True for modules whose apply can lock the operator out (ssh, firewall).
    /// These trigger the timed confirmation guard on apply.
    fn lockout_risk(&self) -> bool {
        false
    }

    /// Inspect the system and report compliance. Read-only.
    async fn check(&self, ctx: &Context) -> Result<Status>;

    /// Compute the changes `apply` would make. Read-only.
    async fn plan(&self, ctx: &Context) -> Result<Vec<Change>>;

    /// Converge the system to the desired state. When `dry_run`, performs no
    /// changes and reports what would happen.
    async fn apply(&self, ctx: &Context, dry_run: bool) -> Result<Report>;

    /// Restore the state captured before the last `apply`.
    async fn rollback(&self, ctx: &Context) -> Result<()>;
}
