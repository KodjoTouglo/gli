//! Exercise Fail2banModule with a mocked, distro-aware runner.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use vpsguard_core::{
    CommandRunner, Config, Context, DistroFamily, Fail2banConfig, Module, Output, Platform, Result,
    State,
};
use vpsguard_modules::Fail2banModule;

/// Package query returns absent; is-enabled returns disabled; else success.
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
            ("dpkg", Some("-s")) | ("rpm", Some("-q")) | ("pacman", Some("-Q")) => (1, ""),
            ("systemctl", Some("is-enabled")) => (1, "disabled"),
            _ => (0, ""),
        };
        Ok(Output {
            code,
            stdout: stdout.into(),
            stderr: String::new(),
        })
    }
}

fn config() -> Config {
    Config {
        fail2ban: Fail2banConfig {
            enabled: true,
            jails: vec!["sshd".into()],
            bantime: Some("10m".into()),
            maxretry: Some(5),
        },
        ..Config::default()
    }
}

fn ctx(root: &std::path::Path, family: DistroFamily) -> (Context, Arc<MockRunner>) {
    let runner = Arc::new(MockRunner::default());
    let ctx = Context::with_parts(config(), root.to_path_buf(), runner.clone())
        .with_platform(Platform::of(family));
    (ctx, runner)
}

#[tokio::test]
async fn debian_installs_configures_and_enables() {
    let tmp = tempfile::tempdir().unwrap();
    let (ctx, runner) = ctx(tmp.path(), DistroFamily::Debian);

    let report = Fail2banModule.apply(&ctx, false).await.unwrap();
    assert!(!report.is_noop());

    let jail = tmp.path().join("etc/fail2ban/jail.d/vpsguard.local");
    let body = tokio::fs::read_to_string(&jail).await.unwrap();
    assert!(body.contains("[sshd]"));
    assert!(body.contains("port = 22"));
    assert!(body.contains("bantime = 10m"));

    let calls = runner.calls.lock().unwrap().clone();
    assert!(calls.iter().any(|c| c == "apt-get install -y fail2ban"));
    assert!(calls
        .iter()
        .any(|c| c.contains("systemctl enable --now fail2ban")));
}

#[tokio::test]
async fn arch_uses_pacman() {
    let tmp = tempfile::tempdir().unwrap();
    let (ctx, runner) = ctx(tmp.path(), DistroFamily::Arch);

    Fail2banModule.apply(&ctx, false).await.unwrap();
    let calls = runner.calls.lock().unwrap().clone();
    assert!(calls.iter().any(|c| c == "pacman -S --noconfirm fail2ban"));
}

#[tokio::test]
async fn unknown_distro_is_not_applicable() {
    let tmp = tempfile::tempdir().unwrap();
    let (ctx, _) = ctx(tmp.path(), DistroFamily::Unknown);
    let status = Fail2banModule.check(&ctx).await.unwrap();
    assert_eq!(status.state, State::NotApplicable);
}

#[tokio::test]
async fn dry_run_writes_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let (ctx, runner) = ctx(tmp.path(), DistroFamily::Debian);

    let report = Fail2banModule.apply(&ctx, true).await.unwrap();
    assert!(report.dry_run);
    assert!(!tmp
        .path()
        .join("etc/fail2ban/jail.d/vpsguard.local")
        .exists());
    assert!(runner
        .calls
        .lock()
        .unwrap()
        .iter()
        .all(|c| !c.contains("install")));
}
