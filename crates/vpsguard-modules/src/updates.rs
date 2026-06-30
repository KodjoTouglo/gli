//! Automatic security updates, per distro family.
//!
//! Debian/Ubuntu: install unattended-upgrades and drop apt.conf.d files to
//! enable periodic upgrades and an optional reboot window. Fedora/Rocky/RHEL:
//! install dnf-automatic, write automatic.conf, and enable its timer. Other
//! families report not-applicable.

use async_trait::async_trait;

use vpsguard_core::{Category, Change, Context, DistroFamily, Module, Report, Result, Status};

const APT_PERIODIC: &str = "/etc/apt/apt.conf.d/20auto-upgrades";
const APT_REBOOT: &str = "/etc/apt/apt.conf.d/51vpsguard-reboot";
const DNF_CONF: &str = "/etc/dnf/automatic.conf";
const DNF_TIMER: &str = "dnf-automatic.timer";

/// Automatic updates module.
#[derive(Debug, Default)]
pub struct UpdatesModule;

#[async_trait]
impl Module for UpdatesModule {
    fn name(&self) -> &str {
        "updates"
    }

    fn summary(&self) -> &str {
        "Enable automatic security updates with an optional reboot window"
    }

    fn category(&self) -> Category {
        Category::System
    }

    async fn check(&self, ctx: &Context) -> Result<Status> {
        if !ctx.config.updates.enabled {
            return Ok(Status::not_applicable("updates disabled in config"));
        }
        match ctx.platform().family {
            DistroFamily::Debian => drift_to_status(debian_drift(ctx).await?),
            DistroFamily::Rhel => drift_to_status(rhel_drift(ctx).await?),
            _ => Ok(Status::not_applicable("no update backend for this distro")),
        }
    }

    async fn plan(&self, ctx: &Context) -> Result<Vec<Change>> {
        if !ctx.config.updates.enabled {
            return Ok(Vec::new());
        }
        let drift = match ctx.platform().family {
            DistroFamily::Debian => debian_drift(ctx).await?,
            DistroFamily::Rhel => rhel_drift(ctx).await?,
            _ => Vec::new(),
        };
        Ok(drift.into_iter().map(Change::command).collect())
    }

    async fn apply(&self, ctx: &Context, dry_run: bool) -> Result<Report> {
        let mut report = Report::new("updates", dry_run);
        if !ctx.config.updates.enabled {
            return Ok(report);
        }

        let drift = match ctx.platform().family {
            DistroFamily::Debian => debian_drift(ctx).await?,
            DistroFamily::Rhel => rhel_drift(ctx).await?,
            _ => return Ok(report),
        };
        if drift.is_empty() {
            return Ok(report);
        }
        if dry_run {
            report.skipped = drift.into_iter().map(Change::command).collect();
            return Ok(report);
        }

        match ctx.platform().family {
            DistroFamily::Debian => debian_apply(ctx, &mut report).await?,
            DistroFamily::Rhel => rhel_apply(ctx, &mut report).await?,
            _ => {}
        }
        Ok(report)
    }

    async fn rollback(&self, ctx: &Context) -> Result<()> {
        match ctx.platform().family {
            DistroFamily::Debian => {
                let _ = ctx.remove(APT_PERIODIC).await;
                let _ = ctx.remove(APT_REBOOT).await;
            }
            DistroFamily::Rhel => {
                let _ = ctx
                    .runner()
                    .run("systemctl", &["disable", "--now", DNF_TIMER])
                    .await;
            }
            _ => {}
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Debian / Ubuntu
// ---------------------------------------------------------------------------

async fn debian_drift(ctx: &Context) -> Result<Vec<String>> {
    let mut drift = Vec::new();
    if !pkg_installed(ctx, "dpkg", &["-s", "unattended-upgrades"]).await {
        drift.push("install unattended-upgrades".into());
    }
    if ctx.read_or_empty(APT_PERIODIC).await? != apt_periodic() {
        drift.push("enable periodic unattended upgrades".into());
    }
    match &ctx.config.updates.auto_reboot {
        Some(time) => {
            if ctx.read_or_empty(APT_REBOOT).await? != apt_reboot(time) {
                drift.push(format!("set auto-reboot window {time}"));
            }
        }
        None => {
            if !ctx.read_or_empty(APT_REBOOT).await?.is_empty() {
                drift.push("disable auto-reboot".into());
            }
        }
    }
    Ok(drift)
}

async fn debian_apply(ctx: &Context, report: &mut Report) -> Result<()> {
    if !pkg_installed(ctx, "dpkg", &["-s", "unattended-upgrades"]).await {
        ctx.runner()
            .run_checked("apt-get", &["install", "-y", "unattended-upgrades"])
            .await?;
        report
            .applied
            .push(Change::command("install unattended-upgrades"));
    }
    write_if_changed(
        ctx,
        APT_PERIODIC,
        &apt_periodic(),
        report,
        "enable periodic upgrades",
    )
    .await?;
    match &ctx.config.updates.auto_reboot {
        Some(time) => {
            let body = apt_reboot(time);
            write_if_changed(ctx, APT_REBOOT, &body, report, "set auto-reboot window").await?;
        }
        None => remove_if_present(ctx, APT_REBOOT, report, "disable auto-reboot").await?,
    }
    Ok(())
}

fn apt_periodic() -> String {
    "APT::Periodic::Update-Package-Lists \"1\";\nAPT::Periodic::Unattended-Upgrade \"1\";\n".into()
}

fn apt_reboot(time: &str) -> String {
    format!(
        "Unattended-Upgrade::Automatic-Reboot \"true\";\n\
         Unattended-Upgrade::Automatic-Reboot-Time \"{time}\";\n"
    )
}

// ---------------------------------------------------------------------------
// Fedora / Rocky / RHEL
// ---------------------------------------------------------------------------

async fn rhel_drift(ctx: &Context) -> Result<Vec<String>> {
    let mut drift = Vec::new();
    if !pkg_installed(ctx, "rpm", &["-q", "dnf-automatic"]).await {
        drift.push("install dnf-automatic".into());
    }
    let want = dnf_conf(ctx.config.updates.auto_reboot.is_some());
    if ctx.read_or_empty(DNF_CONF).await? != want {
        drift.push("configure dnf-automatic".into());
    }
    if !timer_enabled(ctx).await {
        drift.push("enable dnf-automatic.timer".into());
    }
    Ok(drift)
}

async fn rhel_apply(ctx: &Context, report: &mut Report) -> Result<()> {
    if !pkg_installed(ctx, "rpm", &["-q", "dnf-automatic"]).await {
        ctx.runner()
            .run_checked("dnf", &["install", "-y", "dnf-automatic"])
            .await?;
        report
            .applied
            .push(Change::command("install dnf-automatic"));
    }
    let want = dnf_conf(ctx.config.updates.auto_reboot.is_some());
    write_if_changed(ctx, DNF_CONF, &want, report, "configure dnf-automatic").await?;
    ctx.runner()
        .run_checked("systemctl", &["enable", "--now", DNF_TIMER])
        .await?;
    report
        .applied
        .push(Change::command("enable dnf-automatic.timer"));
    Ok(())
}

fn dnf_conf(reboot: bool) -> String {
    let reboot_line = if reboot { "reboot = when-needed\n" } else { "" };
    format!(
        "[commands]\nupgrade_type = security\napply_updates = yes\n{reboot_line}\
         [emitters]\nemit_via = stdio\n"
    )
}

async fn timer_enabled(ctx: &Context) -> bool {
    ctx.runner()
        .run("systemctl", &["is-enabled", DNF_TIMER])
        .await
        .map(|o| o.stdout.trim() == "enabled")
        .unwrap_or(false)
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

fn drift_to_status(drift: Vec<String>) -> Result<Status> {
    if drift.is_empty() {
        Ok(Status::compliant())
    } else {
        Ok(Status::drift(drift.join("; ")))
    }
}

async fn pkg_installed(ctx: &Context, query: &str, args: &[&str]) -> bool {
    ctx.runner()
        .run(query, args)
        .await
        .map(|o| o.success())
        .unwrap_or(false)
}

async fn write_if_changed(
    ctx: &Context,
    rel: &str,
    body: &str,
    report: &mut Report,
    summary: &'static str,
) -> Result<()> {
    if ctx.read_or_empty(rel).await? != body {
        ctx.write(rel, body).await?;
        report.applied.push(Change::command(summary));
    }
    Ok(())
}

async fn remove_if_present(
    ctx: &Context,
    rel: &str,
    report: &mut Report,
    summary: &'static str,
) -> Result<()> {
    if ctx.exists(rel).await? {
        ctx.remove(rel).await?;
        report.applied.push(Change::command(summary));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apt_reboot_embeds_time() {
        let c = apt_reboot("02:00");
        assert!(c.contains("Automatic-Reboot \"true\""));
        assert!(c.contains("Automatic-Reboot-Time \"02:00\""));
    }

    #[test]
    fn dnf_conf_toggles_reboot() {
        assert!(dnf_conf(true).contains("reboot = when-needed"));
        assert!(!dnf_conf(false).contains("reboot ="));
        assert!(dnf_conf(false).contains("apply_updates = yes"));
    }
}
