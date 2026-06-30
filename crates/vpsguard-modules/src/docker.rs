//! Docker runtime module: install the engine, enable it, add users to the group.
//!
//! Install is distro-aware: docker.io on Debian/Ubuntu, the distro docker
//! package on Fedora, the docker-ce repo on Rocky/RHEL, native packages on Arch
//! and openSUSE. Presence is detected with `docker --version`, so the module is
//! idempotent regardless of how Docker was installed. Opt-in: disabled by
//! default. Rollback removes managed group memberships; the engine is left
//! installed.

use async_trait::async_trait;

use vpsguard_core::{
    Category, Change, Context, DistroFamily, Module, Platform, Report, Result, Status,
};

const SERVICE: &str = "docker";

/// Docker runtime module.
#[derive(Debug, Default)]
pub struct DockerModule;

#[async_trait]
impl Module for DockerModule {
    fn name(&self) -> &str {
        "docker"
    }

    fn summary(&self) -> &str {
        "Install Docker, enable the service, add users to the docker group"
    }

    fn category(&self) -> Category {
        Category::Runtime
    }

    async fn check(&self, ctx: &Context) -> Result<Status> {
        if !ctx.config.docker.enabled {
            return Ok(Status::not_applicable("docker disabled in config"));
        }
        if install_plan(ctx.platform()).is_none() {
            return Ok(Status::not_applicable(
                "no docker install path for this distro",
            ));
        }
        Ok(drift_to_status(self_drift(ctx).await))
    }

    async fn plan(&self, ctx: &Context) -> Result<Vec<Change>> {
        if !ctx.config.docker.enabled || install_plan(ctx.platform()).is_none() {
            return Ok(Vec::new());
        }
        Ok(self_drift(ctx)
            .await
            .into_iter()
            .map(Change::command)
            .collect())
    }

    async fn apply(&self, ctx: &Context, dry_run: bool) -> Result<Report> {
        let mut report = Report::new("docker", dry_run);
        if !ctx.config.docker.enabled {
            return Ok(report);
        }
        let Some(plan) = install_plan(ctx.platform()) else {
            return Ok(report);
        };

        let drift = self_drift(ctx).await;
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
            report.applied.push(Change::command("install docker"));
        }

        ctx.runner()
            .run_checked("systemctl", &["enable", "--now", SERVICE])
            .await?;
        report
            .applied
            .push(Change::command("enable docker service"));

        for user in &ctx.config.docker.users {
            if !in_docker_group(ctx, user).await {
                ctx.runner()
                    .run_checked("usermod", &["-aG", "docker", user])
                    .await?;
                report
                    .applied
                    .push(Change::command(format!("add {user} to docker group")));
            }
        }
        Ok(report)
    }

    async fn rollback(&self, ctx: &Context) -> Result<()> {
        for user in &ctx.config.docker.users {
            let _ = ctx.runner().run("gpasswd", &["-d", user, "docker"]).await;
        }
        Ok(())
    }
}

async fn self_drift(ctx: &Context) -> Vec<String> {
    let mut drift = Vec::new();
    if !installed(ctx).await {
        drift.push("install docker".into());
    }
    if !service_enabled(ctx).await {
        drift.push("enable docker service".into());
    }
    for user in &ctx.config.docker.users {
        if !in_docker_group(ctx, user).await {
            drift.push(format!("add {user} to docker group"));
        }
    }
    drift
}

// ---------------------------------------------------------------------------
// Pure logic (no IO), unit-tested below.
// ---------------------------------------------------------------------------

/// Distro-specific install command sequence, or None when unsupported.
fn install_plan(p: &Platform) -> Option<Vec<(&'static str, Vec<&'static str>)>> {
    match p.family {
        DistroFamily::Debian => Some(vec![("apt-get", vec!["install", "-y", "docker.io"])]),
        DistroFamily::Rhel if p.id == "fedora" => {
            Some(vec![("dnf", vec!["install", "-y", "docker"])])
        }
        DistroFamily::Rhel => Some(vec![
            ("dnf", vec!["install", "-y", "dnf-plugins-core"]),
            (
                "dnf",
                vec![
                    "config-manager",
                    "--add-repo",
                    "https://download.docker.com/linux/centos/docker-ce.repo",
                ],
            ),
            (
                "dnf",
                vec![
                    "install",
                    "-y",
                    "docker-ce",
                    "docker-ce-cli",
                    "containerd.io",
                ],
            ),
        ]),
        DistroFamily::Arch => Some(vec![("pacman", vec!["-S", "--noconfirm", "docker"])]),
        DistroFamily::Suse => Some(vec![("zypper", vec!["install", "-y", "docker"])]),
        DistroFamily::Unknown => None,
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

async fn installed(ctx: &Context) -> bool {
    ctx.runner()
        .run("docker", &["--version"])
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

async fn in_docker_group(ctx: &Context, user: &str) -> bool {
    ctx.runner()
        .run("id", &["-nG", user])
        .await
        .map(|o| o.stdout.split_whitespace().any(|g| g == "docker"))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan_first(family: DistroFamily, id: &str) -> Option<(&'static str, Vec<&'static str>)> {
        let p = Platform {
            family,
            id: id.to_string(),
        };
        install_plan(&p).map(|v| v.into_iter().next().unwrap())
    }

    #[test]
    fn debian_uses_docker_io() {
        let (cmd, args) = plan_first(DistroFamily::Debian, "debian").unwrap();
        assert_eq!(cmd, "apt-get");
        assert!(args.contains(&"docker.io"));
    }

    #[test]
    fn fedora_uses_dnf_docker() {
        let (cmd, args) = plan_first(DistroFamily::Rhel, "fedora").unwrap();
        assert_eq!(cmd, "dnf");
        assert!(args.contains(&"docker"));
    }

    #[test]
    fn rocky_adds_docker_ce_repo() {
        let p = Platform {
            family: DistroFamily::Rhel,
            id: "rocky".into(),
        };
        let steps = install_plan(&p).unwrap();
        assert!(steps.iter().any(|(_, a)| a.contains(&"--add-repo")));
        assert!(steps.iter().any(|(_, a)| a.contains(&"docker-ce")));
    }

    #[test]
    fn unknown_distro_has_no_plan() {
        assert!(plan_first(DistroFamily::Unknown, "").is_none());
    }
}
