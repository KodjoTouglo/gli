//! Exercise AppModule (docker-compose runtime and native guard).

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use vpsguard_core::{
    AppConfig, AppRuntime, CommandRunner, Config, Context, Framework, Module, Output, Result, State,
};
use vpsguard_modules::AppModule;

/// `docker compose ps -q` reports nothing running so apply deploys.
#[derive(Default)]
struct MockRunner {
    calls: Mutex<Vec<String>>,
}

#[async_trait]
impl CommandRunner for MockRunner {
    async fn run(&self, command: &str, args: &[&str]) -> Result<Output> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("{command} {}", args.join(" ")));
        Ok(Output {
            code: 0,
            stdout: String::new(),
            stderr: String::new(),
        })
    }
}

fn config(runtime: AppRuntime) -> Config {
    Config {
        app: AppConfig {
            enabled: true,
            framework: Framework::Django,
            runtime,
            repo: Some("https://example.com/me/app.git".into()),
            dir: Some("/srv/app".into()),
            ..AppConfig::default()
        },
        ..Config::default()
    }
}

fn ctx(root: &std::path::Path, cfg: Config) -> (Context, Arc<MockRunner>) {
    let runner = Arc::new(MockRunner::default());
    (
        Context::with_parts(cfg, root.to_path_buf(), runner.clone()),
        runner,
    )
}

#[tokio::test]
async fn docker_runtime_composes_up_when_compose_present() {
    let tmp = tempfile::tempdir().unwrap();
    // Seed an existing checkout with a compose file (mock git creates nothing).
    let dir = tmp.path().join("srv/app");
    tokio::fs::create_dir_all(&dir).await.unwrap();
    tokio::fs::write(dir.join("compose.yaml"), "services: {}\n")
        .await
        .unwrap();
    let (ctx, runner) = ctx(tmp.path(), config(AppRuntime::Docker));

    let report = AppModule.apply(&ctx, false).await.unwrap();
    assert!(!report.is_noop());

    let calls = runner.calls.lock().unwrap().clone();
    assert!(calls
        .iter()
        .any(|c| c == "docker compose --project-directory /srv/app up -d"));
}

#[tokio::test]
async fn docker_runtime_errors_without_compose_file() {
    let tmp = tempfile::tempdir().unwrap();
    // /srv/app absent -> mock clone creates nothing -> no compose file.
    let (ctx, _) = ctx(tmp.path(), config(AppRuntime::Docker));
    let err = AppModule.apply(&ctx, false).await.unwrap_err();
    assert!(err.to_string().contains("no docker compose file"));
}

#[tokio::test]
async fn native_runtime_is_rejected_for_now() {
    let tmp = tempfile::tempdir().unwrap();
    let (ctx, _) = ctx(tmp.path(), config(AppRuntime::Native));
    let err = AppModule.apply(&ctx, false).await.unwrap_err();
    assert!(err
        .to_string()
        .contains("native runtime not yet implemented"));
}

#[tokio::test]
async fn disabled_is_not_applicable() {
    let tmp = tempfile::tempdir().unwrap();
    let (ctx, _) = ctx(tmp.path(), Config::default());
    assert_eq!(
        AppModule.check(&ctx).await.unwrap().state,
        State::NotApplicable
    );
}

#[tokio::test]
async fn no_repo_django_errors_on_apply() {
    let tmp = tempfile::tempdir().unwrap();
    let cfg = Config {
        app: AppConfig {
            enabled: true,
            repo: None,
            dir: Some("/srv/app".into()),
            ..AppConfig::default()
        },
        ..Config::default()
    };
    let (ctx, _) = ctx(tmp.path(), cfg);
    let err = AppModule.apply(&ctx, false).await.unwrap_err();
    assert!(err.to_string().contains("no app.repo set"));
}

#[tokio::test]
async fn wordpress_without_repo_generates_compose_stack() {
    let tmp = tempfile::tempdir().unwrap();
    let cfg = Config {
        app: AppConfig {
            enabled: true,
            framework: Framework::Wordpress,
            runtime: AppRuntime::Docker,
            repo: None,
            dir: Some("/srv/app".into()),
            ..AppConfig::default()
        },
        ..Config::default()
    };
    let (ctx, runner) = ctx(tmp.path(), cfg);

    let report = AppModule.apply(&ctx, false).await.unwrap();
    assert!(!report.is_noop());

    let compose = tmp.path().join("srv/app/compose.yaml");
    let body = tokio::fs::read_to_string(&compose).await.unwrap();
    assert!(body.contains("wordpress:latest"));
    assert!(body.contains("mariadb"));

    let calls = runner.calls.lock().unwrap().clone();
    assert!(calls
        .iter()
        .any(|c| c.contains("compose") && c.contains("up -d")));
    assert!(calls.iter().all(|c| !c.contains("git clone")));
}

#[tokio::test]
async fn native_static_is_served_by_caddy() {
    let tmp = tempfile::tempdir().unwrap();
    let cfg = Config {
        app: AppConfig {
            enabled: true,
            framework: Framework::Static,
            runtime: AppRuntime::Native,
            repo: Some("https://example.com/me/site.git".into()),
            dir: Some("/srv/app".into()),
            ..AppConfig::default()
        },
        ..Config::default()
    };
    let (ctx, runner) = ctx(tmp.path(), cfg);

    let report = AppModule.apply(&ctx, false).await.unwrap();
    assert!(!report.is_noop());
    let calls = runner.calls.lock().unwrap().clone();
    assert!(calls.iter().any(|c| c.starts_with("git clone")));
    // No compose for a static native site.
    assert!(calls.iter().all(|c| !c.contains("compose")));
}

#[tokio::test]
async fn dry_run_runs_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let (ctx, runner) = ctx(tmp.path(), config(AppRuntime::Docker));
    let report = AppModule.apply(&ctx, true).await.unwrap();
    assert!(report.dry_run);
    assert!(runner
        .calls
        .lock()
        .unwrap()
        .iter()
        .all(|c| !c.contains("git clone")));
}
