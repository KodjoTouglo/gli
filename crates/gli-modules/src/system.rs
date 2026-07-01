//! Base system module: hostname, timezone, swap file.
//!
//! The everyday first-boot configuration of a fresh VPS. Each setting is
//! optional; unset settings are left alone. Uses systemd tooling (hostnamectl,
//! timedatectl) and a /swapfile, all through the injected runner and
//! filesystem, so it works locally and over a remote connection alike. Prior
//! hostname/timezone are snapshotted for rollback.

use async_trait::async_trait;

use gli_core::{Category, Change, Context, Module, Report, Result, Status};

const HOSTNAME_FILE: &str = "/etc/hostname";
const FSTAB: &str = "/etc/fstab";
const SWAPFILE: &str = "/swapfile";
const HOSTNAME_BAK: &str = "/etc/gli/system.hostname.bak";
const TIMEZONE_BAK: &str = "/etc/gli/system.timezone.bak";

/// Base system module.
#[derive(Debug, Default)]
pub struct SystemModule;

#[async_trait]
impl Module for SystemModule {
    fn name(&self) -> &str {
        "system"
    }

    fn summary(&self) -> &str {
        "Set hostname, timezone, and a swap file"
    }

    fn category(&self) -> Category {
        Category::System
    }

    async fn check(&self, ctx: &Context) -> Result<Status> {
        let cfg = &ctx.config.system;
        if cfg.hostname.is_none() && cfg.timezone.is_none() && cfg.swap_mb.unwrap_or(0) == 0 {
            return Ok(Status::not_applicable("no base system settings configured"));
        }
        let drift = self_drift(ctx).await?;
        if drift.is_empty() {
            Ok(Status::compliant())
        } else {
            Ok(Status::drift(drift.join("; ")))
        }
    }

    async fn plan(&self, ctx: &Context) -> Result<Vec<Change>> {
        Ok(self_drift(ctx)
            .await?
            .into_iter()
            .map(Change::command)
            .collect())
    }

    async fn apply(&self, ctx: &Context, dry_run: bool) -> Result<Report> {
        let mut report = Report::new("system", dry_run);
        let drift = self_drift(ctx).await?;
        if drift.is_empty() {
            return Ok(report);
        }
        if dry_run {
            report.skipped = drift.into_iter().map(Change::command).collect();
            return Ok(report);
        }

        let cfg = &ctx.config.system;

        if let Some(want) = &cfg.hostname {
            if hostname(ctx).await? != *want {
                ctx.write(HOSTNAME_BAK, &hostname(ctx).await?).await?;
                ctx.runner()
                    .run_checked("hostnamectl", &["set-hostname", want])
                    .await?;
                report
                    .applied
                    .push(Change::command(format!("set hostname to {want}")));
            }
        }

        if let Some(want) = &cfg.timezone {
            if timezone(ctx).await != *want {
                ctx.write(TIMEZONE_BAK, &timezone(ctx).await).await?;
                ctx.runner()
                    .run_checked("timedatectl", &["set-timezone", want])
                    .await?;
                report
                    .applied
                    .push(Change::command(format!("set timezone to {want}")));
            }
        }

        if let Some(mb) = cfg.swap_mb {
            if mb > 0 && !swap_active(ctx).await? {
                ctx.runner()
                    .run_checked("fallocate", &["-l", &format!("{mb}M"), SWAPFILE])
                    .await?;
                ctx.runner()
                    .run_checked("chmod", &["600", SWAPFILE])
                    .await?;
                ctx.runner().run_checked("mkswap", &[SWAPFILE]).await?;
                ctx.runner().run_checked("swapon", &[SWAPFILE]).await?;
                if let Some(updated) = ensure_swap_fstab(&ctx.read_or_empty(FSTAB).await?) {
                    ctx.write(FSTAB, &updated).await?;
                }
                report
                    .applied
                    .push(Change::command(format!("create {mb} MiB swap file")));
            }
        }

        Ok(report)
    }

    async fn rollback(&self, ctx: &Context) -> Result<()> {
        if let Some(prev) = ctx.read(HOSTNAME_BAK).await? {
            let _ = ctx
                .runner()
                .run("hostnamectl", &["set-hostname", prev.trim()])
                .await;
            let _ = ctx.remove(HOSTNAME_BAK).await;
        }
        if let Some(prev) = ctx.read(TIMEZONE_BAK).await? {
            let _ = ctx
                .runner()
                .run("timedatectl", &["set-timezone", prev.trim()])
                .await;
            let _ = ctx.remove(TIMEZONE_BAK).await;
        }
        if swap_active(ctx).await.unwrap_or(false) {
            let _ = ctx.runner().run("swapoff", &[SWAPFILE]).await;
            let _ = ctx.remove(SWAPFILE).await;
            if let Some(stripped) = remove_swap_fstab(&ctx.read_or_empty(FSTAB).await?) {
                let _ = ctx.write(FSTAB, &stripped).await;
            }
        }
        Ok(())
    }

    async fn uninstall(&self, ctx: &Context, _purge: bool) -> Result<Report> {
        // Hostname and timezone are host settings, not removable; only the
        // managed swap file is torn down here.
        let mut report = Report::new("system", false);
        let _ = ctx.runner().run("swapoff", &[SWAPFILE]).await;
        if ctx.remove(SWAPFILE).await.is_ok() {
            if let Some(stripped) = remove_swap_fstab(&ctx.read_or_empty(FSTAB).await?) {
                let _ = ctx.write(FSTAB, &stripped).await;
            }
            report.applied.push(Change::command("remove swap file"));
        }
        Ok(report)
    }
}

async fn self_drift(ctx: &Context) -> Result<Vec<String>> {
    let cfg = &ctx.config.system;
    let mut drift = Vec::new();
    if let Some(want) = &cfg.hostname {
        if hostname(ctx).await? != *want {
            drift.push(format!("set hostname to {want}"));
        }
    }
    if let Some(want) = &cfg.timezone {
        if timezone(ctx).await != *want {
            drift.push(format!("set timezone to {want}"));
        }
    }
    if let Some(mb) = cfg.swap_mb {
        if mb > 0 && !swap_active(ctx).await? {
            drift.push(format!("create {mb} MiB swap file"));
        }
    }
    Ok(drift)
}

// ---------------------------------------------------------------------------
// Pure logic (no IO), unit-tested below.
// ---------------------------------------------------------------------------

/// Add the swapfile line to fstab, or None if it is already present.
fn ensure_swap_fstab(fstab: &str) -> Option<String> {
    if has_swap_line(fstab) {
        return None;
    }
    let mut s = fstab.to_string();
    if !s.is_empty() && !s.ends_with('\n') {
        s.push('\n');
    }
    s.push_str(&format!("{SWAPFILE} none swap sw 0 0\n"));
    Some(s)
}

/// Strip the swapfile line from fstab, or None if there is nothing to remove.
fn remove_swap_fstab(fstab: &str) -> Option<String> {
    if !has_swap_line(fstab) {
        return None;
    }
    let kept: Vec<&str> = fstab
        .lines()
        .filter(|l| l.split_whitespace().next() != Some(SWAPFILE))
        .collect();
    let mut s = kept.join("\n");
    if !s.is_empty() {
        s.push('\n');
    }
    Some(s)
}

fn has_swap_line(fstab: &str) -> bool {
    fstab
        .lines()
        .any(|l| l.split_whitespace().next() == Some(SWAPFILE))
}

// ---------------------------------------------------------------------------
// IO
// ---------------------------------------------------------------------------

async fn hostname(ctx: &Context) -> Result<String> {
    Ok(ctx.read_or_empty(HOSTNAME_FILE).await?.trim().to_string())
}

async fn timezone(ctx: &Context) -> String {
    ctx.runner()
        .run("timedatectl", &["show", "-p", "Timezone", "--value"])
        .await
        .map(|o| o.stdout.trim().to_string())
        .unwrap_or_default()
}

async fn swap_active(ctx: &Context) -> Result<bool> {
    Ok(ctx.read_or_empty("/proc/swaps").await?.contains(SWAPFILE))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adds_swap_line_when_absent() {
        let out = ensure_swap_fstab("UUID=abc / ext4 defaults 0 1\n").unwrap();
        assert!(out.contains("/swapfile none swap sw 0 0"));
        assert!(out.ends_with('\n'));
    }

    #[test]
    fn no_change_when_swap_line_present() {
        let fstab = "/swapfile none swap sw 0 0\n";
        assert!(ensure_swap_fstab(fstab).is_none());
    }

    #[test]
    fn handles_missing_trailing_newline() {
        let out = ensure_swap_fstab("UUID=abc / ext4 defaults 0 1").unwrap();
        assert!(out.contains("defaults 0 1\n/swapfile"));
    }

    #[test]
    fn remove_strips_only_swap_line() {
        let fstab = "UUID=abc / ext4 defaults 0 1\n/swapfile none swap sw 0 0\n";
        let out = remove_swap_fstab(fstab).unwrap();
        assert!(!out.contains("/swapfile"));
        assert!(out.contains("UUID=abc"));
    }

    #[test]
    fn remove_noop_when_absent() {
        assert!(remove_swap_fstab("UUID=abc / ext4 defaults 0 1\n").is_none());
    }
}
