//! Exercise FirewallModule apply/dry-run/rollback with a mocked nft runner.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use vpsguard_core::{
    CommandRunner, Config, Context, FirewallBackend, FirewallConfig, Module, Output, Policy, Result,
};
use vpsguard_modules::FirewallModule;

/// Returns a programmed result for `nft list table`, success otherwise.
struct MockNft {
    calls: Mutex<Vec<String>>,
    list: (i32, String),
}

#[async_trait]
impl CommandRunner for MockNft {
    async fn run(&self, command: &str, args: &[&str]) -> Result<Output> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("{command} {}", args.join(" ")));
        if args.first() == Some(&"list") {
            return Ok(Output {
                code: self.list.0,
                stdout: self.list.1.clone(),
                stderr: String::new(),
            });
        }
        Ok(Output {
            code: 0,
            stdout: String::new(),
            stderr: String::new(),
        })
    }
}

fn config(allow: &[&str]) -> Config {
    Config {
        firewall: FirewallConfig {
            enabled: true,
            backend: FirewallBackend::Nftables,
            default: Policy::Deny,
            allow: allow.iter().map(|s| s.to_string()).collect(),
        },
        ..Config::default()
    }
}

fn ctx(root: &std::path::Path, cfg: Config, list: (i32, String)) -> (Context, Arc<MockNft>) {
    let runner = Arc::new(MockNft {
        calls: Mutex::new(Vec::new()),
        list,
    });
    let ctx = Context::with_parts(cfg, root.to_path_buf(), runner.clone());
    (ctx, runner)
}

#[tokio::test]
async fn apply_creates_table_with_guard_rules() {
    let tmp = tempfile::tempdir().unwrap();
    let (ctx, runner) = ctx(tmp.path(), config(&["80/tcp"]), (1, String::new()));

    let report = FirewallModule.apply(&ctx, false).await.unwrap();
    assert!(!report.is_noop());

    let staged = tmp.path().join("etc/vpsguard/firewall.staged.nft");
    let script = tokio::fs::read_to_string(&staged).await.unwrap();
    assert!(script.contains("policy drop"));
    assert!(script.contains("tcp dport 80 accept"));
    assert!(script.contains("tcp dport 22 accept")); // ssh lockout guard (default port)
    assert!(script.contains("vpsguard:"));

    let calls = runner.calls.lock().unwrap().clone();
    assert!(calls.iter().any(|c| c.starts_with("nft -c -f")));
    assert!(calls.iter().any(|c| c.starts_with("nft -f")));

    let backup = tmp.path().join("etc/vpsguard/firewall.backup.nft");
    let body = tokio::fs::read_to_string(&backup).await.unwrap();
    assert!(body.starts_with("delete table inet vpsguard"));
}

#[tokio::test]
async fn dry_run_writes_no_script_and_runs_no_apply() {
    let tmp = tempfile::tempdir().unwrap();
    let (ctx, runner) = ctx(tmp.path(), config(&["80/tcp"]), (1, String::new()));

    let report = FirewallModule.apply(&ctx, true).await.unwrap();
    assert!(report.dry_run);
    assert!(!report.skipped.is_empty());

    assert!(!tmp.path().join("etc/vpsguard/firewall.staged.nft").exists());
    let calls = runner.calls.lock().unwrap().clone();
    assert!(calls.iter().all(|c| !c.starts_with("nft -f")));
}

#[tokio::test]
async fn rollback_replays_snapshot() {
    let tmp = tempfile::tempdir().unwrap();
    let backup = tmp.path().join("etc/vpsguard/firewall.backup.nft");
    tokio::fs::create_dir_all(backup.parent().unwrap())
        .await
        .unwrap();
    tokio::fs::write(&backup, "delete table inet vpsguard\n")
        .await
        .unwrap();

    let (ctx, runner) = ctx(tmp.path(), config(&[]), (1, String::new()));
    FirewallModule.rollback(&ctx).await.unwrap();

    assert!(runner
        .calls
        .lock()
        .unwrap()
        .iter()
        .any(|c| c.starts_with("nft -f")));
    assert!(!backup.exists());
}

#[tokio::test]
async fn disabled_firewall_is_not_applicable() {
    let tmp = tempfile::tempdir().unwrap();
    let mut cfg = config(&["80/tcp"]);
    cfg.firewall.enabled = false;
    let (ctx, _) = ctx(tmp.path(), cfg, (1, String::new()));

    let status = FirewallModule.check(&ctx).await.unwrap();
    assert_eq!(status.state, vpsguard_core::State::NotApplicable);
}
