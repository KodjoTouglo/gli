//! fail2ban module: install, configure jails, enable the service.
//!
//! Writes a managed drop-in at /etc/fail2ban/jail.d/gli.local enabling the
//! configured jails (the sshd jail tracks the configured SSH port), installs the
//! package via the host package manager, and enables the service. Package query
//! and install are distro-aware; the jail file and service are identical across
//! distros.

use async_trait::async_trait;

use gli_core::{
    Category, Change, Context, DistroFamily, Fail2banConfig, Module, Report, Result, Status,
};

const JAIL_FILE: &str = "/etc/fail2ban/jail.d/gli.local";
const SERVICE: &str = "fail2ban";

/// fail2ban module.
#[derive(Debug, Default)]
pub struct Fail2banModule;

#[async_trait]
impl Module for Fail2banModule {
    fn name(&self) -> &str {
        "fail2ban"
    }

    fn summary(&self) -> &str {
        "Install fail2ban and enable jails (sshd by default)"
    }

    fn category(&self) -> Category {
        Category::Security
    }

    async fn check(&self, ctx: &Context) -> Result<Status> {
        if !ctx.config.fail2ban.enabled {
            return Ok(Status::not_applicable("fail2ban disabled in config"));
        }
        let Some(pkg) = PackageOps::for_host(ctx) else {
            return Ok(Status::not_applicable("no package manager for this distro"));
        };
        Ok(drift_to_status(self_drift(ctx, &pkg).await?))
    }

    async fn plan(&self, ctx: &Context) -> Result<Vec<Change>> {
        if !ctx.config.fail2ban.enabled {
            return Ok(Vec::new());
        }
        let Some(pkg) = PackageOps::for_host(ctx) else {
            return Ok(Vec::new());
        };
        Ok(self_drift(ctx, &pkg)
            .await?
            .into_iter()
            .map(Change::command)
            .collect())
    }

    async fn apply(&self, ctx: &Context, dry_run: bool) -> Result<Report> {
        let mut report = Report::new("fail2ban", dry_run);
        if !ctx.config.fail2ban.enabled {
            return Ok(report);
        }
        let Some(pkg) = PackageOps::for_host(ctx) else {
            return Ok(report);
        };

        let drift = self_drift(ctx, &pkg).await?;
        if drift.is_empty() {
            return Ok(report);
        }
        if dry_run {
            report.skipped = drift.into_iter().map(Change::command).collect();
            return Ok(report);
        }

        if !pkg.installed(ctx).await {
            ctx.runner()
                .run_checked(pkg.install.0, &pkg.install.1)
                .await?;
            report.applied.push(Change::command("install fail2ban"));
        }

        let want = jail_config(&ctx.config.fail2ban, ctx.config.ssh.port);
        if ctx.read_or_empty(JAIL_FILE).await? != want {
            ctx.write(JAIL_FILE, &want).await?;
            report.applied.push(Change::command("write fail2ban jails"));
        }

        ctx.runner()
            .run_checked("systemctl", &["enable", "--now", SERVICE])
            .await?;
        ctx.runner()
            .run_checked("systemctl", &["reload-or-restart", SERVICE])
            .await?;
        report
            .applied
            .push(Change::command("enable and reload fail2ban"));
        Ok(report)
    }

    async fn rollback(&self, ctx: &Context) -> Result<()> {
        let _ = ctx.remove(JAIL_FILE).await;
        let _ = ctx
            .runner()
            .run("systemctl", &["reload-or-restart", SERVICE])
            .await;
        Ok(())
    }

    async fn uninstall(&self, ctx: &Context, purge: bool) -> Result<Report> {
        let mut report = Report::new("fail2ban", false);
        crate::common::disable_service(ctx, SERVICE).await;
        let _ = ctx.remove(JAIL_FILE).await;
        crate::common::remove_pkg(ctx, "fail2ban", purge).await;
        report.applied.push(Change::command("remove fail2ban"));
        if purge {
            let _ = ctx.runner().run("rm", &["-rf", "/var/lib/fail2ban"]).await;
            report.applied.push(Change::command("purge fail2ban data"));
        }
        Ok(report)
    }
}

async fn self_drift(ctx: &Context, pkg: &PackageOps) -> Result<Vec<String>> {
    let mut drift = Vec::new();
    if !pkg.installed(ctx).await {
        drift.push("install fail2ban".into());
    }
    let want = jail_config(&ctx.config.fail2ban, ctx.config.ssh.port);
    if ctx.read_or_empty(JAIL_FILE).await? != want {
        drift.push("configure fail2ban jails".into());
    }
    if !service_enabled(ctx).await {
        drift.push("enable fail2ban service".into());
    }
    Ok(drift)
}

// ---------------------------------------------------------------------------
// Pure logic (no IO), unit-tested below.
// ---------------------------------------------------------------------------

/// Render the managed jail drop-in. The sshd jail tracks the SSH port.
fn jail_config(cfg: &Fail2banConfig, ssh_port: u16) -> String {
    let mut s = String::from("[DEFAULT]\n");
    if let Some(b) = &cfg.bantime {
        s.push_str(&format!("bantime = {b}\n"));
    }
    if let Some(m) = cfg.maxretry {
        s.push_str(&format!("maxretry = {m}\n"));
    }
    for jail in &cfg.jails {
        s.push_str(&format!("\n[{jail}]\nenabled = true\n"));
        if jail == "sshd" {
            s.push_str(&format!("port = {ssh_port}\n"));
        }
    }
    s
}

/// Distro-specific package query and install commands.
struct PackageOps {
    query: (&'static str, Vec<&'static str>),
    install: (&'static str, Vec<&'static str>),
}

impl PackageOps {
    fn for_host(ctx: &Context) -> Option<Self> {
        let manager = ctx.platform().package_manager()?;
        let query = match ctx.platform().family {
            DistroFamily::Debian => ("dpkg", vec!["-s", "fail2ban"]),
            DistroFamily::Arch => ("pacman", vec!["-Q", "fail2ban"]),
            DistroFamily::Rhel | DistroFamily::Suse => ("rpm", vec!["-q", "fail2ban"]),
            DistroFamily::Unknown => return None,
        };
        let install = match manager {
            "pacman" => ("pacman", vec!["-S", "--noconfirm", "fail2ban"]),
            other => (other, vec!["install", "-y", "fail2ban"]),
        };
        Some(Self { query, install })
    }

    async fn installed(&self, ctx: &Context) -> bool {
        ctx.runner()
            .run(self.query.0, &self.query.1)
            .await
            .map(|o| o.success())
            .unwrap_or(false)
    }
}

// ---------------------------------------------------------------------------
// IO
// ---------------------------------------------------------------------------

fn drift_to_status(drift: Vec<String>) -> Status {
    if drift.is_empty() {
        Status::compliant()
    } else {
        Status::drift(drift.join("; "))
    }
}

async fn service_enabled(ctx: &Context) -> bool {
    ctx.runner()
        .run("systemctl", &["is-enabled", SERVICE])
        .await
        .map(|o| o.stdout.trim() == "enabled")
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> Fail2banConfig {
        Fail2banConfig {
            enabled: true,
            jails: vec!["sshd".into()],
            bantime: Some("10m".into()),
            maxretry: Some(5),
        }
    }

    #[test]
    fn sshd_jail_tracks_port() {
        let c = jail_config(&cfg(), 2222);
        assert!(c.contains("[sshd]"));
        assert!(c.contains("enabled = true"));
        assert!(c.contains("port = 2222"));
    }

    #[test]
    fn defaults_embed_bantime_and_maxretry() {
        let c = jail_config(&cfg(), 22);
        assert!(c.contains("bantime = 10m"));
        assert!(c.contains("maxretry = 5"));
    }

    #[test]
    fn omits_unset_defaults() {
        let bare = Fail2banConfig {
            enabled: true,
            jails: vec!["sshd".into()],
            bantime: None,
            maxretry: None,
        };
        let c = jail_config(&bare, 22);
        assert!(!c.contains("bantime"));
        assert!(!c.contains("maxretry"));
    }

    #[test]
    fn non_sshd_jail_has_no_port() {
        let c = jail_config(
            &Fail2banConfig {
                enabled: true,
                jails: vec!["nginx-http-auth".into()],
                bantime: None,
                maxretry: None,
            },
            22,
        );
        assert!(c.contains("[nginx-http-auth]"));
        assert!(!c.contains("port ="));
    }
}
