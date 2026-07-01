//! Value types exchanged between modules and the front-ends.

/// Outcome of a module's compliance check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Status {
    pub state: State,
    /// Human-readable summary (e.g. "PasswordAuthentication=yes, expected no").
    pub detail: String,
}

impl Status {
    pub fn compliant() -> Self {
        Self {
            state: State::Compliant,
            detail: "compliant".into(),
        }
    }

    pub fn drift(detail: impl Into<String>) -> Self {
        Self {
            state: State::Drift,
            detail: detail.into(),
        }
    }

    pub fn not_applicable(detail: impl Into<String>) -> Self {
        Self {
            state: State::NotApplicable,
            detail: detail.into(),
        }
    }

    pub fn is_compliant(&self) -> bool {
        self.state == State::Compliant
    }
}

/// Coarse compliance state, used to colour the dashboard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    /// System matches desired config.
    Compliant,
    /// System differs from desired config; changes are pending.
    Drift,
    /// Check could not complete.
    Error,
    /// Module does not apply on this host.
    NotApplicable,
}

/// A single pending or applied modification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    pub kind: ChangeKind,
    /// One-line description shown in the plan.
    pub summary: String,
    /// Prior value, when known (for diff rendering).
    pub before: Option<String>,
    /// Desired value, when applicable.
    pub after: Option<String>,
}

impl Change {
    pub fn modify(
        summary: impl Into<String>,
        before: impl Into<String>,
        after: impl Into<String>,
    ) -> Self {
        Self {
            kind: ChangeKind::Modify,
            summary: summary.into(),
            before: Some(before.into()),
            after: Some(after.into()),
        }
    }

    pub fn command(summary: impl Into<String>) -> Self {
        Self {
            kind: ChangeKind::Command,
            summary: summary.into(),
            before: None,
            after: None,
        }
    }
}

/// Nature of a [`Change`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeKind {
    Create,
    Modify,
    Delete,
    /// A side-effecting command (e.g. `systemctl restart sshd`).
    Command,
}

/// Result of an `apply` run.
#[derive(Debug, Clone)]
pub struct Report {
    pub module: String,
    /// Changes that were performed (empty when `dry_run`).
    pub applied: Vec<Change>,
    /// Changes that were planned but intentionally not performed.
    pub skipped: Vec<Change>,
    pub dry_run: bool,
}

impl Report {
    pub fn new(module: impl Into<String>, dry_run: bool) -> Self {
        Self {
            module: module.into(),
            applied: Vec::new(),
            skipped: Vec::new(),
            dry_run,
        }
    }

    /// True when no changes were performed (already compliant, or dry-run).
    pub fn is_noop(&self) -> bool {
        self.applied.is_empty()
    }
}
