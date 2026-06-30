//! Exercise CaddyModule against a tempdir root with a mocked runner.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use vpsguard_core::{
    CaddyConfig, CaddySite, CommandRunner, Config, Context, DistroFamily, Module, Output, Platform,
    Result, State,
};
use vpsguard_modules::CaddyModule;

/// caddy version and is-enabled report absent; everything else succeeds.
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
            ("caddy", Some("version")) => (1, ""),
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

fn config(enabled: bool, sites: Vec<CaddySite>) -> Config {
    Config {
        caddy: CaddyConfig { enabled, sites },
        ..Config::default()
    }
}

fn site() -> CaddySite {
    CaddySite {
        domain: "example.com".into(),
        reverse_proxy: Some("localhost:8080".into()),
        root: None,
    }
}

fn ctx(root: &std::path::Path, cfg: Config, family: DistroFamily) -> (Context, Arc<MockRunner>) {
    let runner = Arc::new(MockRunner::default());
    let ctx =
        Context::with_parts(cfg, root.to_path_buf(), runner.clone()).with_platform(Platform {
            family,
            id: "debian".into(),
        });
    (ctx, runner)
}

#[tokio::test]
async fn installs_writes_caddyfile_and_reloads() {
    let tmp = tempfile::tempdir().unwrap();
    let (ctx, runner) = ctx(tmp.path(), config(true, vec![site()]), DistroFamily::Debian);

    let report = CaddyModule.apply(&ctx, false).await.unwrap();
    assert!(!report.is_noop());

    let caddyfile = tmp.path().join("etc/caddy/Caddyfile");
    let body = tokio::fs::read_to_string(&caddyfile).await.unwrap();
    assert!(body.contains("example.com {"));
    assert!(body.contains("reverse_proxy localhost:8080"));

    let calls = runner.calls.lock().unwrap().clone();
    assert!(calls.iter().any(|c| c == "apt-get install -y caddy"));
    assert!(calls
        .iter()
        .any(|c| c.contains("systemctl enable --now caddy")));
}

#[tokio::test]
async fn disabled_is_not_applicable() {
    let tmp = tempfile::tempdir().unwrap();
    let (ctx, _) = ctx(
        tmp.path(),
        config(false, vec![site()]),
        DistroFamily::Debian,
    );
    assert_eq!(
        CaddyModule.check(&ctx).await.unwrap().state,
        State::NotApplicable
    );
}

#[tokio::test]
async fn enabled_without_sites_is_not_applicable() {
    let tmp = tempfile::tempdir().unwrap();
    let (ctx, _) = ctx(tmp.path(), config(true, vec![]), DistroFamily::Debian);
    assert_eq!(
        CaddyModule.check(&ctx).await.unwrap().state,
        State::NotApplicable
    );
}

#[tokio::test]
async fn dry_run_writes_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let (ctx, runner) = ctx(tmp.path(), config(true, vec![site()]), DistroFamily::Debian);

    let report = CaddyModule.apply(&ctx, true).await.unwrap();
    assert!(report.dry_run);
    assert!(!tmp.path().join("etc/caddy/Caddyfile").exists());
    assert!(runner
        .calls
        .lock()
        .unwrap()
        .iter()
        .all(|c| !c.contains("apt-get install -y caddy")));
}

#[tokio::test]
async fn rollback_restores_previous_caddyfile() {
    let tmp = tempfile::tempdir().unwrap();
    let caddyfile = tmp.path().join("etc/caddy/Caddyfile");
    tokio::fs::create_dir_all(caddyfile.parent().unwrap())
        .await
        .unwrap();
    tokio::fs::write(&caddyfile, "old.com {\n}\n")
        .await
        .unwrap();

    let (ctx, _) = ctx(tmp.path(), config(true, vec![site()]), DistroFamily::Debian);
    CaddyModule.apply(&ctx, false).await.unwrap();
    // New config written.
    assert!(tokio::fs::read_to_string(&caddyfile)
        .await
        .unwrap()
        .contains("example.com"));

    CaddyModule.rollback(&ctx).await.unwrap();
    assert_eq!(
        tokio::fs::read_to_string(&caddyfile).await.unwrap(),
        "old.com {\n}\n"
    );
}
