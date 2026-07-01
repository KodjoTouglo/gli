//! Exercise DockerModule across distro families with a mocked runner.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use gli_core::{
    CommandRunner, Config, Context, DistroFamily, DockerConfig, Module, Output, Platform, Result,
    State,
};
use gli_modules::DockerModule;

/// docker --version and is-enabled report absent; id reports no docker group.
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
        let (code, stdout) = match (command, args.first().copied()) {
            ("docker", _) => (1, ""),
            ("systemctl", Some("is-enabled")) => (1, "disabled"),
            ("id", _) => (0, "deploy wheel"),
            _ => (0, ""),
        };
        Ok(Output {
            code,
            stdout: stdout.into(),
            stderr: String::new(),
        })
    }
}

fn config(enabled: bool) -> Config {
    Config {
        docker: DockerConfig {
            enabled,
            users: vec!["deploy".into()],
        },
        ..Config::default()
    }
}

fn ctx(family: DistroFamily, enabled: bool) -> (Context, Arc<MockRunner>) {
    let runner = Arc::new(MockRunner::default());
    let ctx = Context::with_parts(
        config(enabled),
        std::path::PathBuf::from("/"),
        runner.clone(),
    )
    .with_platform(Platform {
        family,
        id: String::new(),
    });
    (ctx, runner)
}

#[tokio::test]
async fn debian_installs_enables_and_adds_group() {
    let (ctx, runner) = ctx(DistroFamily::Debian, true);

    let report = DockerModule.apply(&ctx, false).await.unwrap();
    assert!(!report.is_noop());

    let calls = runner.calls.lock().unwrap().clone();
    assert!(calls.iter().any(|c| c == "apt-get install -y docker.io"));
    assert!(calls.iter().any(|c| c == "systemctl enable --now docker"));
    assert!(calls.iter().any(|c| c == "usermod -aG docker deploy"));
}

#[tokio::test]
async fn disabled_is_not_applicable() {
    let (ctx, _) = ctx(DistroFamily::Debian, false);
    let status = DockerModule.check(&ctx).await.unwrap();
    assert_eq!(status.state, State::NotApplicable);
}

#[tokio::test]
async fn unknown_distro_is_not_applicable() {
    let (ctx, _) = ctx(DistroFamily::Unknown, true);
    let status = DockerModule.check(&ctx).await.unwrap();
    assert_eq!(status.state, State::NotApplicable);
}

#[tokio::test]
async fn dry_run_runs_no_install() {
    let (ctx, runner) = ctx(DistroFamily::Debian, true);
    let report = DockerModule.apply(&ctx, true).await.unwrap();
    assert!(report.dry_run);
    assert!(!report.skipped.is_empty());
    let calls = runner.calls.lock().unwrap().clone();
    assert!(calls.iter().all(|c| !c.contains("install")));
    assert!(calls.iter().all(|c| !c.starts_with("usermod")));
}

#[tokio::test]
async fn rollback_removes_group_membership() {
    let (ctx, runner) = ctx(DistroFamily::Debian, true);
    DockerModule.rollback(&ctx).await.unwrap();
    assert!(runner
        .calls
        .lock()
        .unwrap()
        .iter()
        .any(|c| c == "gpasswd -d deploy docker"));
}
