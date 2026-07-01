//! Exercise SystemModule against a tempdir root with a mocked runner.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use gli_core::{CommandRunner, Config, Context, Module, Output, Result, State, SystemConfig};
use gli_modules::SystemModule;

/// Records calls; `timedatectl show` reports UTC so timezone drift is visible.
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
        let stdout = if command == "timedatectl" && args.first() == Some(&"show") {
            "UTC"
        } else {
            ""
        };
        Ok(Output {
            code: 0,
            stdout: stdout.into(),
            stderr: String::new(),
        })
    }
}

fn config() -> Config {
    Config {
        system: SystemConfig {
            hostname: Some("my-vps".into()),
            timezone: Some("Europe/Paris".into()),
            swap_mb: Some(1024),
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
async fn sets_hostname_timezone_and_swap() {
    let tmp = tempfile::tempdir().unwrap();
    // Empty /etc/hostname and no /proc/swaps -> all drift.
    let (ctx, runner) = ctx(tmp.path(), config());

    let report = SystemModule.apply(&ctx, false).await.unwrap();
    assert!(!report.is_noop());

    let calls = runner.calls.lock().unwrap().clone();
    assert!(calls.iter().any(|c| c == "hostnamectl set-hostname my-vps"));
    assert!(calls
        .iter()
        .any(|c| c == "timedatectl set-timezone Europe/Paris"));
    assert!(calls.iter().any(|c| c.starts_with("fallocate -l 1024M")));
    assert!(calls.iter().any(|c| c.starts_with("swapon")));

    // fstab got the swap line.
    let fstab = tokio::fs::read_to_string(tmp.path().join("etc/fstab"))
        .await
        .unwrap();
    assert!(fstab.contains("/swapfile none swap sw 0 0"));
}

#[tokio::test]
async fn unset_settings_are_not_applicable() {
    let tmp = tempfile::tempdir().unwrap();
    let (ctx, _) = ctx(tmp.path(), Config::default());
    assert_eq!(
        SystemModule.check(&ctx).await.unwrap().state,
        State::NotApplicable
    );
}

#[tokio::test]
async fn compliant_when_hostname_matches_and_no_swap() {
    let tmp = tempfile::tempdir().unwrap();
    tokio::fs::create_dir_all(tmp.path().join("etc"))
        .await
        .unwrap();
    tokio::fs::write(tmp.path().join("etc/hostname"), "host1\n")
        .await
        .unwrap();
    let cfg = Config {
        system: SystemConfig {
            hostname: Some("host1".into()),
            timezone: None,
            swap_mb: None,
        },
        ..Config::default()
    };
    let (ctx, _) = ctx(tmp.path(), cfg);
    assert!(SystemModule.check(&ctx).await.unwrap().is_compliant());
}

#[tokio::test]
async fn dry_run_changes_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let (ctx, runner) = ctx(tmp.path(), config());
    let report = SystemModule.apply(&ctx, true).await.unwrap();
    assert!(report.dry_run);
    assert!(!report.skipped.is_empty());
    assert!(runner
        .calls
        .lock()
        .unwrap()
        .iter()
        .all(|c| !c.starts_with("hostnamectl set-hostname")));
}
