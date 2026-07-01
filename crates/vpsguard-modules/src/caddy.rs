//! Caddy reverse-proxy module: install Caddy, manage the Caddyfile, reload.
//!
//! Each configured site becomes a Caddyfile block; Caddy obtains and renews
//! Let's Encrypt certificates automatically for public domains, so this single
//! module covers web server, reverse proxy, and TLS. Install is distro-aware
//! (Caddy's apt repo on Debian, distro/COPR packages elsewhere). The Caddyfile
//! is snapshotted before each change for rollback.

use async_trait::async_trait;

use std::path::Path;

use vpsguard_core::{
    CaddySite, Category, Change, Context, DistroFamily, Module, Platform, Report, Result, Status,
};

use crate::common::with_suffix;

const CADDYFILE: &str = "/etc/caddy/Caddyfile";
const BACKUP_SUFFIX: &str = ".vpsguard.bak";
const SERVICE: &str = "caddy";

/// Caddy reverse-proxy module.
#[derive(Debug, Default)]
pub struct CaddyModule;

#[async_trait]
impl Module for CaddyModule {
    fn name(&self) -> &str {
        "caddy"
    }

    fn summary(&self) -> &str {
        "Install Caddy reverse proxy with automatic HTTPS"
    }

    fn category(&self) -> Category {
        Category::Network
    }

    async fn check(&self, ctx: &Context) -> Result<Status> {
        if !ctx.config.caddy.enabled {
            return Ok(Status::not_applicable("caddy disabled in config"));
        }
        if ctx.config.caddy.sites.is_empty() {
            return Ok(Status::not_applicable("no caddy sites configured"));
        }
        if install_plan(ctx.platform()).is_none() {
            return Ok(Status::not_applicable(
                "no caddy install path for this distro",
            ));
        }
        Ok(drift_to_status(self_drift(ctx).await?))
    }

    async fn plan(&self, ctx: &Context) -> Result<Vec<Change>> {
        if !ready(ctx) {
            return Ok(Vec::new());
        }
        Ok(self_drift(ctx)
            .await?
            .into_iter()
            .map(Change::command)
            .collect())
    }

    async fn apply(&self, ctx: &Context, dry_run: bool) -> Result<Report> {
        let mut report = Report::new("caddy", dry_run);
        if !ready(ctx) {
            return Ok(report);
        }
        let plan = install_plan(ctx.platform()).expect("checked by ready()");

        let drift = self_drift(ctx).await?;
        if drift.is_empty() {
            return Ok(report);
        }
        if dry_run {
            report.skipped = drift.into_iter().map(Change::command).collect();
            return Ok(report);
        }

        if !installed(ctx).await {
            for (cmd, args) in &plan {
                ctx.runner().run_checked(cmd, args).await?;
            }
            report.applied.push(Change::command("install caddy"));
        }

        let want = caddyfile(&ctx.config.caddy.sites);
        let current = ctx.read_or_empty(CADDYFILE).await?;
        if current != want {
            let backup = with_suffix(Path::new(CADDYFILE), BACKUP_SUFFIX);
            ctx.write(&backup, &current).await?;
            ctx.write(CADDYFILE, &want).await?;
            report.applied.push(Change::command("write Caddyfile"));
        }

        ctx.runner()
            .run_checked("systemctl", &["enable", "--now", SERVICE])
            .await?;
        ctx.runner()
            .run_checked("systemctl", &["reload-or-restart", SERVICE])
            .await?;
        report
            .applied
            .push(Change::command("enable and reload caddy"));
        Ok(report)
    }

    async fn rollback(&self, ctx: &Context) -> Result<()> {
        let backup = with_suffix(Path::new(CADDYFILE), BACKUP_SUFFIX);
        if let Some(saved) = ctx.read(&backup).await? {
            ctx.write(CADDYFILE, &saved).await?;
            let _ = ctx.remove(&backup).await;
            let _ = ctx
                .runner()
                .run("systemctl", &["reload-or-restart", SERVICE])
                .await;
        }
        Ok(())
    }

    async fn uninstall(&self, ctx: &Context, purge: bool) -> Result<Report> {
        let mut report = Report::new("caddy", false);
        crate::common::disable_service(ctx, SERVICE).await;
        let _ = ctx.remove(CADDYFILE).await;
        crate::common::remove_pkg(ctx, "caddy", purge).await;
        report.applied.push(Change::command("remove caddy"));
        if purge {
            // Certificates and site data.
            let _ = ctx.runner().run("rm", &["-rf", "/var/lib/caddy"]).await;
            report.applied.push(Change::command("purge caddy data"));
        }
        Ok(report)
    }
}

/// Enabled, has sites, and a known install path.
fn ready(ctx: &Context) -> bool {
    ctx.config.caddy.enabled
        && !ctx.config.caddy.sites.is_empty()
        && install_plan(ctx.platform()).is_some()
}

async fn self_drift(ctx: &Context) -> Result<Vec<String>> {
    let mut drift = Vec::new();
    if !installed(ctx).await {
        drift.push("install caddy".into());
    }
    let want = caddyfile(&ctx.config.caddy.sites);
    if ctx.read_or_empty(CADDYFILE).await? != want {
        let n = ctx.config.caddy.sites.len();
        drift.push(format!("configure {n} caddy site(s)"));
    }
    if !service_enabled(ctx).await {
        drift.push("enable caddy service".into());
    }
    Ok(drift)
}

// ---------------------------------------------------------------------------
// Pure logic (no IO), unit-tested below.
// ---------------------------------------------------------------------------

/// Render a Caddyfile from the configured sites.
fn caddyfile(sites: &[CaddySite]) -> String {
    let mut s = String::new();
    for site in sites {
        s.push_str(&format!("{} {{\n", site.domain));
        if let Some(up) = &site.reverse_proxy {
            s.push_str(&format!("\treverse_proxy {up}\n"));
        } else if let Some(root) = &site.root {
            s.push_str(&format!("\troot * {root}\n\tfile_server\n"));
        }
        s.push_str("}\n");
    }
    s
}

/// Distro-specific Caddy install command sequence, or None when unsupported.
fn install_plan(p: &Platform) -> Option<Vec<(&'static str, Vec<&'static str>)>> {
    match p.family {
        DistroFamily::Debian => Some(vec![
            (
                "apt-get",
                vec![
                    "install",
                    "-y",
                    "debian-keyring",
                    "debian-archive-keyring",
                    "apt-transport-https",
                    "curl",
                ],
            ),
            ("sh", vec!["-c", DEB_KEY]),
            ("sh", vec!["-c", DEB_REPO]),
            ("apt-get", vec!["update"]),
            ("apt-get", vec!["install", "-y", "caddy"]),
        ]),
        DistroFamily::Rhel if p.id == "fedora" => {
            Some(vec![("dnf", vec!["install", "-y", "caddy"])])
        }
        DistroFamily::Rhel => Some(vec![
            ("dnf", vec!["install", "-y", "dnf-plugins-core"]),
            ("dnf", vec!["copr", "enable", "-y", "@caddy/caddy"]),
            ("dnf", vec!["install", "-y", "caddy"]),
        ]),
        DistroFamily::Arch => Some(vec![("pacman", vec!["-S", "--noconfirm", "caddy"])]),
        DistroFamily::Suse => Some(vec![("zypper", vec!["install", "-y", "caddy"])]),
        DistroFamily::Unknown => None,
    }
}

const DEB_KEY: &str = "curl -1sLf 'https://dl.cloudsmith.io/public/caddy/stable/gpg.key' \
    | gpg --dearmor -o /usr/share/keyrings/caddy-stable-archive-keyring.gpg";
const DEB_REPO: &str = "curl -1sLf 'https://dl.cloudsmith.io/public/caddy/stable/debian.deb.txt' \
    | tee /etc/apt/sources.list.d/caddy-stable.list";

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

async fn installed(ctx: &Context) -> bool {
    ctx.runner()
        .run("caddy", &["version"])
        .await
        .map(|o| o.success())
        .unwrap_or(false)
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

    fn proxy(domain: &str, up: &str) -> CaddySite {
        CaddySite {
            domain: domain.into(),
            reverse_proxy: Some(up.into()),
            root: None,
        }
    }

    #[test]
    fn reverse_proxy_block() {
        let c = caddyfile(&[proxy("example.com", "localhost:8080")]);
        assert!(c.contains("example.com {"));
        assert!(c.contains("reverse_proxy localhost:8080"));
    }

    #[test]
    fn static_site_block() {
        let site = CaddySite {
            domain: "static.example.com".into(),
            reverse_proxy: None,
            root: Some("/var/www/site".into()),
        };
        let c = caddyfile(&[site]);
        assert!(c.contains("root * /var/www/site"));
        assert!(c.contains("file_server"));
    }

    #[test]
    fn multiple_sites_each_get_a_block() {
        let c = caddyfile(&[proxy("a.com", "localhost:1"), proxy("b.com", "localhost:2")]);
        assert_eq!(c.matches('{').count(), 2);
        assert!(c.contains("a.com {"));
        assert!(c.contains("b.com {"));
    }

    #[test]
    fn debian_uses_caddy_apt_repo() {
        let p = Platform {
            family: DistroFamily::Debian,
            id: "debian".into(),
        };
        let steps = install_plan(&p).unwrap();
        assert!(steps
            .iter()
            .any(|(_, a)| a.iter().any(|x| x.contains("cloudsmith"))));
        assert!(steps.last().unwrap().1.contains(&"caddy"));
    }

    #[test]
    fn rocky_uses_copr() {
        let p = Platform {
            family: DistroFamily::Rhel,
            id: "rocky".into(),
        };
        let steps = install_plan(&p).unwrap();
        assert!(steps.iter().any(|(_, a)| a.contains(&"copr")));
    }

    #[test]
    fn unknown_distro_has_no_plan() {
        let p = Platform {
            family: DistroFamily::Unknown,
            id: String::new(),
        };
        assert!(install_plan(&p).is_none());
    }
}
