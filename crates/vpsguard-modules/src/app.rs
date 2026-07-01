//! Application deploy module: clone a repo (or generate a stack) and run it.
//!
//! Opt-in, disabled by default. The docker runtime runs the repo's compose file
//! (framework-agnostic). WordPress with no repo generates a self-contained
//! WordPress + MariaDB compose stack. The native runtime serves a static site
//! via Caddy; native runtimes for the dynamic frameworks are planned. The
//! domain and database are wired to Caddy and the db modules in Config::resolve.

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
        "Deploy an app (Django, Laravel, Node, WordPress, ...) via compose"
    }

    fn category(&self) -> Category {
        Category::App
    }

    async fn check(&self, ctx: &Context) -> Result<Status> {
        if !ctx.config.app.enabled {
            return Ok(Status::not_applicable("app deploy disabled in config"));
        }
        Ok(drift_to_status(self_drift(ctx).await))
    }

    async fn plan(&self, ctx: &Context) -> Result<Vec<Change>> {
        if !ctx.config.app.enabled {
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

        let drift = self_drift(ctx).await;
        if drift.is_empty() {
            return Ok(report);
        }
        if dry_run {
            report.skipped = drift.into_iter().map(Change::command).collect();
            return Ok(report);
        }

        let dir = dir(ctx);

        // WordPress with no repo: generate a WordPress + MariaDB compose stack.
        if cfg.framework == Framework::Wordpress && cfg.repo.is_none() {
            ctx.write(format!("{dir}/compose.yaml"), WORDPRESS_COMPOSE)
                .await?;
            report
                .applied
                .push(Change::command("write WordPress compose stack"));
            compose_up_cmd(ctx, &dir).await?;
            report.applied.push(Change::command("docker compose up -d"));
            return Ok(report);
        }

        // Native runtime: static sites are served by Caddy, nothing to run.
        if cfg.runtime == AppRuntime::Native {
            if cfg.framework == Framework::Static {
                ensure_checkout(ctx, &dir, &mut report).await?;
                report
                    .applied
                    .push(Change::command("static site served by Caddy"));
                return Ok(report);
            }
            return Err(Error::Module {
                module: "app".into(),
                message: format!(
                    "native runtime not yet implemented for {}; use runtime = \"docker\"",
                    framework_name(cfg.framework)
                ),
            });
        }

        // Docker runtime: deploy the repo's compose file.
        ensure_checkout(ctx, &dir, &mut report).await?;
        if !has_compose_file(ctx, &dir).await {
            return Err(Error::Module {
                module: "app".into(),
                message: format!(
                    "no docker compose file in {dir}; add compose.yaml or set runtime = \"native\""
                ),
            });
        }
        compose_up_cmd(ctx, &dir).await?;
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
    if running(ctx).await {
        Vec::new()
    } else {
        vec![format!(
            "deploy {} app",
            framework_name(ctx.config.app.framework)
        )]
    }
}

/// Whether the app is already up. Static native sites are "up" once checked out;
/// everything else is up when its compose project has running containers.
async fn running(ctx: &Context) -> bool {
    let cfg = &ctx.config.app;
    let dir = dir(ctx);
    if cfg.runtime == AppRuntime::Native && cfg.framework == Framework::Static {
        ctx.exists(&dir).await.unwrap_or(false)
    } else {
        compose_up(ctx, &dir).await
    }
}

/// Clone the repo (or pull if present); require the dir when there is no repo.
async fn ensure_checkout(ctx: &Context, dir: &str, report: &mut Report) -> Result<()> {
    match &ctx.config.app.repo {
        Some(repo) if !ctx.exists(dir).await? => {
            ctx.runner()
                .run_checked("git", &["clone", repo, dir])
                .await?;
            report
                .applied
                .push(Change::command(format!("clone {repo} to {dir}")));
        }
        Some(_) => {
            let _ = ctx.runner().run("git", &["-C", dir, "pull"]).await;
            report.applied.push(Change::command(format!("pull {dir}")));
        }
        None if !ctx.exists(dir).await? => {
            return Err(Error::Module {
                module: "app".into(),
                message: format!("no app.repo set and {dir} does not exist"),
            });
        }
        None => {}
    }
    Ok(())
}

async fn compose_up_cmd(ctx: &Context, dir: &str) -> Result<()> {
    ctx.runner()
        .run_checked(
            "docker",
            &["compose", "--project-directory", dir, "up", "-d"],
        )
        .await
        .map(|_| ())
}

/// A self-contained WordPress + MariaDB compose stack, published on port 8080
/// to match the default reverse-proxy wiring. Change the passwords before use.
const WORDPRESS_COMPOSE: &str = r#"services:
  db:
    image: mariadb:11
    restart: unless-stopped
    environment:
      MARIADB_DATABASE: wordpress
      MARIADB_USER: wordpress
      MARIADB_PASSWORD: wordpress
      MARIADB_ROOT_PASSWORD: wordpress
    volumes:
      - db:/var/lib/mysql
  wordpress:
    image: wordpress:latest
    restart: unless-stopped
    depends_on:
      - db
    ports:
      - "8080:80"
    environment:
      WORDPRESS_DB_HOST: db
      WORDPRESS_DB_USER: wordpress
      WORDPRESS_DB_PASSWORD: wordpress
      WORDPRESS_DB_NAME: wordpress
    volumes:
      - wp:/var/www/html
volumes:
  db:
  wp:
"#;

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
        Framework::Php => "PHP",
        Framework::Wordpress => "WordPress",
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
