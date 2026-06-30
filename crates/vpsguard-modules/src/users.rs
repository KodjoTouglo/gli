//! User management module: create accounts, grant sudo, install SSH keys.
//!
//! Sudo is granted with a per-user file under /etc/sudoers.d (validated by
//! `visudo -cf`), which is portable across distros where the sudo group name
//! differs. SSH keys are merged into the user's authorized_keys, never removing
//! keys the operator added by hand. Account creation uses `useradd`.

use async_trait::async_trait;

use vpsguard_core::{Category, Change, Context, Error, Module, Report, Result, Status, UserConfig};

use crate::common::write;

const PASSWD: &str = "/etc/passwd";
const SUDOERS_DIR: &str = "/etc/sudoers.d";

/// User management module.
#[derive(Debug, Default)]
pub struct UsersModule;

#[async_trait]
impl Module for UsersModule {
    fn name(&self) -> &str {
        "users"
    }

    fn summary(&self) -> &str {
        "Create users, grant sudo via sudoers.d, install SSH keys"
    }

    fn category(&self) -> Category {
        Category::Security
    }

    async fn check(&self, ctx: &Context) -> Result<Status> {
        if ctx.config.users.is_empty() {
            return Ok(Status::not_applicable("no users configured"));
        }
        let mut drift = Vec::new();
        for (name, spec) in &ctx.config.users {
            drift.extend(user_drift(ctx, name, spec).await?);
        }
        if drift.is_empty() {
            Ok(Status::compliant())
        } else {
            Ok(Status::drift(drift.join("; ")))
        }
    }

    async fn plan(&self, ctx: &Context) -> Result<Vec<Change>> {
        let mut changes = Vec::new();
        for (name, spec) in &ctx.config.users {
            for d in user_drift(ctx, name, spec).await? {
                changes.push(Change::command(d));
            }
        }
        Ok(changes)
    }

    async fn apply(&self, ctx: &Context, dry_run: bool) -> Result<Report> {
        let mut report = Report::new("users", dry_run);
        if ctx.config.users.is_empty() {
            return Ok(report);
        }

        let mut planned = Vec::new();
        for (name, spec) in &ctx.config.users {
            for d in user_drift(ctx, name, spec).await? {
                planned.push(Change::command(d));
            }
        }
        if planned.is_empty() {
            return Ok(report);
        }
        if dry_run {
            report.skipped = planned;
            return Ok(report);
        }

        for (name, spec) in &ctx.config.users {
            apply_user(ctx, name, spec, &mut report).await?;
        }
        Ok(report)
    }

    async fn rollback(&self, ctx: &Context) -> Result<()> {
        // Revoke managed sudo grants. Accounts and keys are additive and left
        // in place (removing them risks locking out the operator).
        for name in ctx.config.users.keys() {
            let file = ctx.path(sudoers_file(name));
            let _ = tokio::fs::remove_file(&file).await;
        }
        Ok(())
    }
}

async fn apply_user(
    ctx: &Context,
    name: &str,
    spec: &UserConfig,
    report: &mut Report,
) -> Result<()> {
    if !user_exists(ctx, name).await? {
        ctx.runner()
            .run_checked("useradd", &["-m", "-s", "/bin/bash", name])
            .await?;
        report
            .applied
            .push(Change::command(format!("create user {name}")));
    }

    if !spec.ssh_keys.is_empty() {
        let path = ctx.path(authorized_keys(name));
        let existing = read_or_empty(&path).await?;
        let merged = merge_keys(&existing, &spec.ssh_keys);
        if merged != existing {
            write(&path, &merged).await?;
            let ssh_dir = ctx.path(format!("/home/{name}/.ssh"));
            let ssh_dir = ssh_dir.to_string_lossy().into_owned();
            let ak = path.to_string_lossy().into_owned();
            ctx.runner()
                .run_checked("chmod", &["700", &ssh_dir])
                .await?;
            ctx.runner().run_checked("chmod", &["600", &ak]).await?;
            ctx.runner()
                .run_checked("chown", &["-R", &format!("{name}:{name}"), &ssh_dir])
                .await?;
            report
                .applied
                .push(Change::command(format!("install SSH keys for {name}")));
        }
    }

    let sudoers = ctx.path(sudoers_file(name));
    if spec.sudo {
        let want = sudoers_content(name);
        if read_or_empty(&sudoers).await? != want {
            write(&sudoers, &want).await?;
            let path = sudoers.to_string_lossy().into_owned();
            let check = ctx.runner().run("visudo", &["-cf", &path]).await?;
            if !check.success() {
                let _ = tokio::fs::remove_file(&sudoers).await;
                return Err(Error::Safety(format!(
                    "sudoers file for {name} rejected by visudo: {}",
                    check.stderr.trim()
                )));
            }
            report
                .applied
                .push(Change::command(format!("grant sudo to {name}")));
        }
    } else if tokio::fs::try_exists(&sudoers).await.unwrap_or(false) {
        tokio::fs::remove_file(&sudoers)
            .await
            .map_err(|e| Error::io(sudoers.display().to_string(), e))?;
        report
            .applied
            .push(Change::command(format!("revoke sudo from {name}")));
    }

    Ok(())
}

/// Drift descriptions for one user (empty when compliant).
async fn user_drift(ctx: &Context, name: &str, spec: &UserConfig) -> Result<Vec<String>> {
    let mut drift = Vec::new();
    if !user_exists(ctx, name).await? {
        drift.push(format!("create user {name}"));
    }
    if !spec.ssh_keys.is_empty() {
        let existing = read_or_empty(&ctx.path(authorized_keys(name))).await?;
        let missing = missing_keys(&existing, &spec.ssh_keys);
        if missing > 0 {
            drift.push(format!("install {missing} SSH key(s) for {name}"));
        }
    }
    let sudoers = read_or_empty(&ctx.path(sudoers_file(name))).await?;
    if spec.sudo && sudoers != sudoers_content(name) {
        drift.push(format!("grant sudo to {name}"));
    } else if !spec.sudo && !sudoers.is_empty() {
        drift.push(format!("revoke sudo from {name}"));
    }
    Ok(drift)
}

// ---------------------------------------------------------------------------
// Pure logic (no IO), unit-tested below.
// ---------------------------------------------------------------------------

fn sudoers_file(name: &str) -> String {
    format!("{SUDOERS_DIR}/vpsguard-{name}")
}

fn authorized_keys(name: &str) -> String {
    format!("/home/{name}/.ssh/authorized_keys")
}

fn sudoers_content(name: &str) -> String {
    format!("{name} ALL=(ALL:ALL) ALL\n")
}

/// Count configured keys not already present (by trimmed-line equality).
fn missing_keys(existing: &str, want: &[String]) -> usize {
    let have: Vec<&str> = existing.lines().map(str::trim).collect();
    want.iter().filter(|k| !have.contains(&k.trim())).count()
}

/// Append any missing keys to `existing`, preserving operator-added keys.
fn merge_keys(existing: &str, want: &[String]) -> String {
    let have: Vec<String> = existing.lines().map(|l| l.trim().to_string()).collect();
    let mut out = existing.to_string();
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    for key in want {
        let k = key.trim();
        if !have.iter().any(|h| h == k) {
            out.push_str(k);
            out.push('\n');
        }
    }
    out
}

// ---------------------------------------------------------------------------
// IO
// ---------------------------------------------------------------------------

async fn user_exists(ctx: &Context, name: &str) -> Result<bool> {
    let passwd = read_or_empty(&ctx.path(PASSWD)).await?;
    let prefix = format!("{name}:");
    Ok(passwd.lines().any(|l| l.starts_with(&prefix)))
}

async fn read_or_empty(path: &std::path::Path) -> Result<String> {
    match tokio::fs::read_to_string(path).await {
        Ok(c) => Ok(c),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(e) => Err(Error::io(path.display().to_string(), e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys() -> Vec<String> {
        vec![
            "ssh-ed25519 AAAAkeyA user@a".into(),
            "ssh-ed25519 AAAAkeyB user@b".into(),
        ]
    }

    #[test]
    fn missing_keys_counts_absent() {
        assert_eq!(missing_keys("", &keys()), 2);
        assert_eq!(missing_keys("ssh-ed25519 AAAAkeyA user@a\n", &keys()), 1);
        assert_eq!(
            missing_keys(
                "ssh-ed25519 AAAAkeyA user@a\nssh-ed25519 AAAAkeyB user@b\n",
                &keys()
            ),
            0
        );
    }

    #[test]
    fn merge_appends_only_missing() {
        let merged = merge_keys("ssh-ed25519 AAAAkeyA user@a\n", &keys());
        assert_eq!(merged.matches("AAAAkeyA").count(), 1);
        assert_eq!(merged.matches("AAAAkeyB").count(), 1);
    }

    #[test]
    fn merge_preserves_operator_keys() {
        let merged = merge_keys("ssh-rsa OPERATOR root@host\n", &keys());
        assert!(merged.contains("OPERATOR root@host"));
        assert!(merged.contains("AAAAkeyA"));
    }

    #[test]
    fn merge_is_idempotent() {
        let once = merge_keys("", &keys());
        let twice = merge_keys(&once, &keys());
        assert_eq!(once, twice);
    }

    #[test]
    fn merge_handles_missing_trailing_newline() {
        let merged = merge_keys("ssh-rsa OP root@h", &keys());
        assert!(merged.contains("OP root@h\nssh-ed25519 AAAAkeyA"));
    }

    #[test]
    fn sudoers_content_is_stable() {
        assert_eq!(sudoers_content("deploy"), "deploy ALL=(ALL:ALL) ALL\n");
    }
}
