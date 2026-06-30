//! Redis module: install the server and enable it.
//!
//! Opt-in, disabled by default. Installs the distro package and enables the
//! service; Redis listens on localhost by default. Package and service names
//! differ across distros. Rollback is a no-op (the data store is left in place).

use async_trait::async_trait;

use vpsguard_core::{Category, Change, Context, DistroFamily, Module, Report, Result, Status};

/// Redis module.
#[derive(Debug, Default)]
pub struct RedisModule;

#[async_trait]
impl Module for RedisModule {
    fn name(&self) -> &str {
        "redis"
    }

    fn summary(&self) -> &str {
        "Install Redis and enable its service"
    }

    fn category(&self) -> Category {
        Category::Runtime
    }

    async fn check(&self, ctx: &Context) -> Result<Status> {
        if !ctx.config.redis.enabled {
            return Ok(Status::not_applicable("redis disabled in config"));
        }
        let Some(pkg) = packaging(ctx) else {
            return Ok(Status::not_applicable(
                "no redis install path for this distro",
            ));
        };
        Ok(drift_to_status(self_drift(ctx, &pkg).await))
    }

    async fn plan(&self, ctx: &Context) -> Result<Vec<Change>> {
        let (Some(pkg), true) = (packaging(ctx), ctx.config.redis.enabled) else {
            return Ok(Vec::new());
        };
        Ok(self_drift(ctx, &pkg)
            .await
            .into_iter()
            .map(Change::command)
            .collect())
    }

    async fn apply(&self, ctx: &Context, dry_run: bool) -> Result<Report> {
        let mut report = Report::new("redis", dry_run);
        if !ctx.config.redis.enabled {
            return Ok(report);
        }
        let Some(pkg) = packaging(ctx) else {
            return Ok(report);
        };
        let drift = self_drift(ctx, &pkg).await;
        if drift.is_empty() {
            return Ok(report);
        }
        if dry_run {
            report.skipped = drift.into_iter().map(Change::command).collect();
            return Ok(report);
        }

        if !installed(ctx).await {
            ctx.runner()
                .run_checked(pkg.install.0, &pkg.install.1)
                .await?;
            report.applied.push(Change::command("install redis"));
        }
        ctx.runner()
            .run_checked("systemctl", &["enable", "--now", pkg.service])
            .await?;
        report.applied.push(Change::command("enable redis service"));
        Ok(report)
    }

    async fn rollback(&self, _ctx: &Context) -> Result<()> {
        Ok(())
    }
}

async fn self_drift(ctx: &Context, pkg: &Packaging) -> Vec<String> {
    let mut drift = Vec::new();
    if !installed(ctx).await {
        drift.push("install redis".into());
    }
    if !service_enabled(ctx, pkg.service).await {
        drift.push("enable redis service".into());
    }
    drift
}

// ---------------------------------------------------------------------------
// Distro packaging
// ---------------------------------------------------------------------------

struct Packaging {
    install: (&'static str, Vec<&'static str>),
    service: &'static str,
}

fn packaging(ctx: &Context) -> Option<Packaging> {
    match ctx.platform().family {
        DistroFamily::Debian => Some(Packaging {
            install: ("apt-get", vec!["install", "-y", "redis-server"]),
            service: "redis-server",
        }),
        DistroFamily::Rhel => Some(Packaging {
            install: ("dnf", vec!["install", "-y", "redis"]),
            service: "redis",
        }),
        DistroFamily::Arch => Some(Packaging {
            install: ("pacman", vec!["-S", "--noconfirm", "redis"]),
            service: "redis",
        }),
        DistroFamily::Suse => Some(Packaging {
            install: ("zypper", vec!["install", "-y", "redis"]),
            service: "redis",
        }),
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
        .run("redis-server", &["--version"])
        .await
        .map(|o| o.success())
        .unwrap_or(false)
}

async fn service_enabled(ctx: &Context, service: &str) -> bool {
    ctx.runner()
        .run("systemctl", &["is-enabled", service])
        .await
        .map(|o| o.stdout.trim() == "enabled")
        .unwrap_or(false)
}
