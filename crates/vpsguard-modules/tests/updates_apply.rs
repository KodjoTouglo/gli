//! Exercise UpdatesModule across distro families with a mocked runner.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use vpsguard_core::{
    CommandRunner, Config, Context, DistroFamily, Module, Output, Platform, Result, State,
    UpdatesConfig,
};
use vpsguard_modules::UpdatesModule;

/// Reports packages as absent (non-zero on dpkg/rpm query), success otherwise.
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
        let absent = matches!(command, "dpkg" | "rpm");
        Ok(Output {
            code: if absent { 1 } else { 0 },
            stdout: String::new(),
            stderr: String::new(),
        })
    }
}

fn config(reboot: Option<&str>) -> Config {
    Config {
        updates: UpdatesConfig {
            enabled: true,
            auto_reboot: reboot.map(str::to_string),
        },
        ..Config::default()
    }
}

fn ctx(
    root: &std::path::Path,
    family: DistroFamily,
    reboot: Option<&str>,
) -> (Context, Arc<MockRunner>) {
    let runner = Arc::new(MockRunner::default());
    let ctx = Context::with_parts(config(reboot), root.to_path_buf(), runner.clone())
        .with_platform(Platform::of(family));
    (ctx, runner)
}

#[tokio::test]
async fn debian_installs_and_writes_apt_conf() {
    let tmp = tempfile::tempdir().unwrap();
    let (ctx, runner) = ctx(tmp.path(), DistroFamily::Debian, Some("02:00"));

    let report = UpdatesModule.apply(&ctx, false).await.unwrap();
    assert!(!report.is_noop());

    let periodic = tmp.path().join("etc/apt/apt.conf.d/20auto-upgrades");
    assert!(tokio::fs::read_to_string(&periodic)
        .await
        .unwrap()
        .contains("Unattended-Upgrade \"1\""));
    let reboot = tmp.path().join("etc/apt/apt.conf.d/51vpsguard-reboot");
    assert!(tokio::fs::read_to_string(&reboot)
        .await
        .unwrap()
        .contains("02:00"));

    let calls = runner.calls.lock().unwrap().clone();
    assert!(calls
        .iter()
        .any(|c| c.contains("apt-get install -y unattended-upgrades")));
}

#[tokio::test]
async fn rhel_installs_writes_conf_and_enables_timer() {
    let tmp = tempfile::tempdir().unwrap();
    let (ctx, runner) = ctx(tmp.path(), DistroFamily::Rhel, Some("02:00"));

    UpdatesModule.apply(&ctx, false).await.unwrap();

    let conf = tmp.path().join("etc/dnf/automatic.conf");
    let body = tokio::fs::read_to_string(&conf).await.unwrap();
    assert!(body.contains("apply_updates = yes"));
    assert!(body.contains("reboot = when-needed"));

    let calls = runner.calls.lock().unwrap().clone();
    assert!(calls
        .iter()
        .any(|c| c.contains("dnf install -y dnf-automatic")));
    assert!(calls
        .iter()
        .any(|c| c.contains("systemctl enable --now dnf-automatic.timer")));
}

#[tokio::test]
async fn unsupported_family_is_not_applicable() {
    let tmp = tempfile::tempdir().unwrap();
    let (ctx, _) = ctx(tmp.path(), DistroFamily::Arch, None);
    let status = UpdatesModule.check(&ctx).await.unwrap();
    assert_eq!(status.state, State::NotApplicable);
}

#[tokio::test]
async fn dry_run_writes_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let (ctx, runner) = ctx(tmp.path(), DistroFamily::Debian, Some("02:00"));

    let report = UpdatesModule.apply(&ctx, true).await.unwrap();
    assert!(report.dry_run);
    assert!(!tmp
        .path()
        .join("etc/apt/apt.conf.d/20auto-upgrades")
        .exists());
    // Only package queries ran, no install.
    let calls = runner.calls.lock().unwrap().clone();
    assert!(calls.iter().all(|c| !c.contains("apt-get install")));
}
