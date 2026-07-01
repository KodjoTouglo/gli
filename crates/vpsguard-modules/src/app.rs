//! Application deploy module: clone a repo and run it.
//!
//! Opt-in, disabled by default. The runtime is chosen in config: `docker` runs
//! the repo's compose file (framework-agnostic), `native` installs the
//! framework runtime directly (planned). The framework (Django/Laravel) and
//! database are recorded for the native path and for future reverse-proxy
//! wiring. This v1 implements the docker-compose runtime.

use async_trait::async_trait;

use vpsguard_core::{
    AppRuntime, Category, Change, Context, Error, Framework, Module, Report, Result, Status,
};

const DEFAULT_DIR: &str = "/srv/app";

/// Application deploy module.
#[derive(Debug, Default)]
pub struct AppModule;

#[async_trait]
impl Module for AppModule {
    fn name(&self) -> &str {
        "app"
    }

    fn summary(&self) -> &str {
        "Deploy a Django/Laravel app via docker compose or natively"
    }

    fn category(&self) -> Category {
        Category::App
    }

    async fn check(&self, ctx: &Context) -> Result<Status> {
        let cfg = &ctx.config.app;
        if !cfg.enabled {
            return Ok(Status::not_applicable("app deploy disabled in config"));
        }
        if cfg.repo.is_none() {
            return Ok(Status::not_applicable("no [app].repo configured"));
        }
        Ok(drift_to_status(self_drift(ctx).await))
    }

    async fn plan(&self, ctx: &Context) -> Result<Vec<Change>> {
        if !ctx.config.app.enabled || ctx.config.app.repo.is_none() {
            return Ok(Vec::new());
        }
        Ok(self_drift(ctx)
            .await
            .into_iter()
            .map(Change::command)
            .collect())
    }

    async fn apply(&self, ctx: &Context, dry_run: bool) -> Result<Report> {
        let mut report = Report::new("app", dry_run);
        let cfg = &ctx.config.app;
        if !cfg.enabled {
            return Ok(report);
        }
        let Some(repo) = &cfg.repo else {
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

        if cfg.runtime == AppRuntime::Native {
            return Err(Error::Module {
                module: "app".into(),
                message: format!(
                    "native runtime not yet implemented for {}; use runtime = \"docker\"",
                    framework_name(cfg.framework)
                ),
            });
        }

        let dir = dir(ctx);
        if !ctx.exists(&dir).await? {
            ctx.runner()
                .run_checked("git", &["clone", repo, &dir])
                .await?;
            report
                .applied
                .push(Change::command(format!("clone {repo} to {dir}")));
        } else {
            let _ = ctx.runner().run("git", &["-C", &dir, "pull"]).await;
            report.applied.push(Change::command(format!("pull {dir}")));
        }

        if !has_compose_file(ctx, &dir).await {
            return Err(Error::Module {
                module: "app".into(),
                message: format!(
                    "no docker compose file in {dir}; add compose.yaml or set runtime = \"native\""
                ),
            });
        }

        ctx.runner()
            .run_checked(
                "docker",
                &["compose", "--project-directory", &dir, "up", "-d"],
            )
            .await?;
        report.applied.push(Change::command("docker compose up -d"));
        Ok(report)
    }

    async fn rollback(&self, ctx: &Context) -> Result<()> {
        if ctx.config.app.runtime == AppRuntime::Docker {
            let dir = dir(ctx);
            let _ = ctx
                .runner()
                .run("docker", &["compose", "--project-directory", &dir, "down"])
                .await;
        }
        Ok(())
    }

    async fn uninstall(&self, ctx: &Context, purge: bool) -> Result<Report> {
        let mut report = Report::new("app", false);
        let dir = dir(ctx);
        if ctx.config.app.runtime == AppRuntime::Docker {
            // -v removes named volumes (data) only when purging.
            let mut args = vec!["compose", "--project-directory", dir.as_str(), "down"];
            if purge {
                args.push("-v");
            }
            let _ = ctx.runner().run("docker", &args).await;
        }
        report.applied.push(Change::command("stop app"));
        if purge {
            let _ = ctx.runner().run("rm", &["-rf", &dir]).await;
            report
                .applied
                .push(Change::command(format!("purge checkout {dir}")));
        }
        Ok(report)
    }
}

async fn self_drift(ctx: &Context) -> Vec<String> {
    let cfg = &ctx.config.app;
    if cfg.runtime == AppRuntime::Native {
        return vec![format!(
            "deploy {} app (native runtime not yet implemented)",
            framework_name(cfg.framework)
        )];
    }
    let mut drift = Vec::new();
    let dir = dir(ctx);
    if !ctx.exists(&dir).await.unwrap_or(false) {
        if let Some(repo) = &cfg.repo {
            drift.push(format!("clone {repo}"));
        }
    }
    if !compose_up(ctx, &dir).await {
        drift.push("docker compose up".into());
    }
    drift
}

// ---------------------------------------------------------------------------
// Pure logic
// ---------------------------------------------------------------------------

fn dir(ctx: &Context) -> String {
    ctx.config
        .app
        .dir
        .clone()
        .unwrap_or_else(|| DEFAULT_DIR.to_string())
}

fn framework_name(f: Framework) -> &'static str {
    match f {
        Framework::Django => "Django",
        Framework::Laravel => "Laravel",
        Framework::Node => "Node/Next.js",
        Framework::Fastapi => "FastAPI",
        Framework::Rails => "Rails",
        Framework::Generic => "Generic",
        Framework::Static => "Static",
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

/// Whether the checkout contains a docker compose file.
async fn has_compose_file(ctx: &Context, dir: &str) -> bool {
    for name in [
        "compose.yaml",
        "compose.yml",
        "docker-compose.yml",
        "docker-compose.yaml",
    ] {
        if ctx.exists(format!("{dir}/{name}")).await.unwrap_or(false) {
            return true;
        }
    }
    false
}

/// True when the compose project has running containers.
async fn compose_up(ctx: &Context, dir: &str) -> bool {
    ctx.runner()
        .run(
            "docker",
            &["compose", "--project-directory", dir, "ps", "-q"],
        )
        .await
        .map(|o| o.success() && !o.stdout.trim().is_empty())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use vpsguard_core::{AppConfig, Config};

    fn ctx_with(app: AppConfig) -> Context {
        Context::system(Config {
            app,
            ..Config::default()
        })
    }

    #[test]
    fn dir_defaults_to_srv_app() {
        let ctx = ctx_with(AppConfig::default());
        assert_eq!(dir(&ctx), "/srv/app");
    }

    #[test]
    fn dir_honours_override() {
        let ctx = ctx_with(AppConfig {
            dir: Some("/opt/myapp".into()),
            ..AppConfig::default()
        });
        assert_eq!(dir(&ctx), "/opt/myapp");
    }

    #[test]
    fn framework_names() {
        assert_eq!(framework_name(Framework::Django), "Django");
        assert_eq!(framework_name(Framework::Laravel), "Laravel");
        assert_eq!(framework_name(Framework::Node), "Node/Next.js");
        assert_eq!(framework_name(Framework::Fastapi), "FastAPI");
        assert_eq!(framework_name(Framework::Rails), "Rails");
    }
}
