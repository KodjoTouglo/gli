//! PostgreSQL module: install the server, enable it, create databases.
//!
//! Opt-in, disabled by default. Installs the distro package (initialising the
//! cluster on RHEL), enables the service, and creates any listed databases that
//! are missing. Rollback does not drop databases, to avoid destroying data.
//! Supported on Debian/Ubuntu and Fedora/Rocky/RHEL.

use async_trait::async_trait;

use vpsguard_core::{Category, Change, Context, DistroFamily, Module, Report, Result, Status};

const SERVICE: &str = "postgresql";

/// PostgreSQL module.
#[derive(Debug, Default)]
pub struct PostgresModule;

#[async_trait]
impl Module for PostgresModule {
    fn name(&self) -> &str {
        "postgres"
    }

    fn summary(&self) -> &str {
        "Install PostgreSQL, enable it, create databases"
    }

    fn category(&self) -> Category {
        Category::Runtime
    }

    async fn check(&self, ctx: &Context) -> Result<Status> {
        if !ctx.config.postgres.enabled {
            return Ok(Status::not_applicable("postgres disabled in config"));
        }
        if !supported(ctx) {
            return Ok(Status::not_applicable(
                "no postgres install path for this distro",
            ));
        }
        Ok(drift_to_status(self_drift(ctx).await))
    }

    async fn plan(&self, ctx: &Context) -> Result<Vec<Change>> {
        if !ctx.config.postgres.enabled || !supported(ctx) {
            return Ok(Vec::new());
        }
        Ok(self_drift(ctx)
            .await
            .into_iter()
            .map(Change::command)
            .collect())
    }

    async fn apply(&self, ctx: &Context, dry_run: bool) -> Result<Report> {
        let mut report = Report::new("postgres", dry_run);
        if !ctx.config.postgres.enabled || !supported(ctx) {
            return Ok(report);
        }
        let drift = self_drift(ctx).await;
        if drift.is_empty() {
            return Ok(report);
        }
        if dry_run {
            report.skipped = drift.into_iter().map(Change::command).collect();
            return Ok(report);
        }

        if !installed(ctx).await {
            match ctx.platform().family {
                DistroFamily::Debian => {
                    ctx.runner()
                        .run_checked("apt-get", &["install", "-y", "postgresql"])
                        .await?;
                }
                DistroFamily::Rhel => {
                    ctx.runner()
                        .run_checked("dnf", &["install", "-y", "postgresql-server"])
                        .await?;
                    // initdb is a no-op error if the cluster already exists.
                    let _ = ctx.runner().run("postgresql-setup", &["--initdb"]).await;
                }
                _ => {}
            }
            report.applied.push(Change::command("install postgresql"));
        }

        ctx.runner()
            .run_checked("systemctl", &["enable", "--now", SERVICE])
            .await?;

        for db in &ctx.config.postgres.databases {
            if !database_exists(ctx, db).await {
                ctx.runner()
                    .run_checked("sudo", &["-u", "postgres", "createdb", db])
                    .await?;
                report
                    .applied
                    .push(Change::command(format!("create database {db}")));
            }
        }
        Ok(report)
    }

    async fn rollback(&self, _ctx: &Context) -> Result<()> {
        // Databases are not dropped on rollback (data preservation).
        Ok(())
    }
}

async fn self_drift(ctx: &Context) -> Vec<String> {
    let mut drift = Vec::new();
    if !installed(ctx).await {
        drift.push("install postgresql".into());
    }
    if !service_enabled(ctx).await {
        drift.push("enable postgresql service".into());
    }
    for db in &ctx.config.postgres.databases {
        if !database_exists(ctx, db).await {
            drift.push(format!("create database {db}"));
        }
    }
    drift
}

// ---------------------------------------------------------------------------
// IO
// ---------------------------------------------------------------------------

fn supported(ctx: &Context) -> bool {
    matches!(
        ctx.platform().family,
        DistroFamily::Debian | DistroFamily::Rhel
    )
}

fn drift_to_status(drift: Vec<String>) -> Status {
    if drift.is_empty() {
        Status::compliant()
    } else {
        Status::drift(drift.join("; "))
    }
}

async fn installed(ctx: &Context) -> bool {
    ctx.runner()
        .run("psql", &["--version"])
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

async fn database_exists(ctx: &Context, db: &str) -> bool {
    let query = format!("SELECT 1 FROM pg_database WHERE datname='{db}'");
    ctx.runner()
        .run("sudo", &["-u", "postgres", "psql", "-tAc", &query])
        .await
        .map(|o| o.stdout.trim() == "1")
        .unwrap_or(false)
}
