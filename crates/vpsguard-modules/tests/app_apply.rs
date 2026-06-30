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
async fn missing_repo_is_not_applicable() {
    let tmp = tempfile::tempdir().unwrap();
    let cfg = Config {
        app: AppConfig {
            enabled: true,
            repo: None,
            ..AppConfig::default()
        },
        ..Config::default()
    };
    let (ctx, _) = ctx(tmp.path(), cfg);
    assert_eq!(
        AppModule.check(&ctx).await.unwrap().state,
        State::NotApplicable
    );
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
