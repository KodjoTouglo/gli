//! End-to-end exercise of `SshModule::apply`/`rollback` against a tempdir root
//! with a mocked command runner (no real `sshd`/`systemctl` invoked).

use std::sync::Arc;
use std::sync::Mutex;

use async_trait::async_trait;
use vpsguard_core::{
    CommandRunner, Config, Context, Module, Output, Profile, Result, SshConfig, State,
};
use vpsguard_modules::SshModule;

/// Records invocations and returns success for everything.
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

fn config() -> Config {
    Config {
        profile: Profile::Balanced,
        ssh: SshConfig {
            port: 2222,
            permit_root_login: false,
            password_auth: false,
            modern_ciphers: false,
        },
    }
}

async fn write_sshd(root: &std::path::Path, body: &str) -> std::path::PathBuf {
    let path = root.join("etc/ssh/sshd_config");
    tokio::fs::create_dir_all(path.parent().unwrap())
        .await
        .unwrap();
    tokio::fs::write(&path, body).await.unwrap();
    path
}

#[tokio::test]
async fn apply_rewrites_validates_and_restarts() {
    let tmp = tempfile::tempdir().unwrap();
    let path = write_sshd(tmp.path(), "Port 22\nPermitRootLogin yes\n").await;

    let runner = Arc::new(MockRunner::default());
    let ctx = Context::with_parts(config(), tmp.path().to_path_buf(), runner.clone());
    let module = SshModule;

    // Drift detected first.
    let status = module.check(&ctx).await.unwrap();
    assert_eq!(status.state, State::Drift);

    // Apply for real.
    let report = module.apply(&ctx, false).await.unwrap();
    assert!(!report.is_noop());

    let written = tokio::fs::read_to_string(&path).await.unwrap();
    assert!(written.contains("Port 2222"));
    assert!(written.contains("PermitRootLogin no"));

    // Validated before restart.
    let calls = runner.calls.lock().unwrap().clone();
    assert!(calls.iter().any(|c| c.starts_with("sshd -t")));
    assert!(calls.iter().any(|c| c.contains("systemctl restart ssh")));

    // Backup snapshot exists.
    let backup = tmp.path().join("etc/ssh/sshd_config.vpsguard.bak");
    assert!(backup.exists());

    // Now compliant.
    assert!(module.check(&ctx).await.unwrap().is_compliant());
}

#[tokio::test]
async fn apply_is_idempotent() {
    let tmp = tempfile::tempdir().unwrap();
    write_sshd(tmp.path(), "Port 22\n").await;
    let runner = Arc::new(MockRunner::default());
    let ctx = Context::with_parts(config(), tmp.path().to_path_buf(), runner.clone());
    let module = SshModule;

    module.apply(&ctx, false).await.unwrap();
    // Second apply must be a no-op (no changes, no restart).
    let second = module.apply(&ctx, false).await.unwrap();
    assert!(second.is_noop(), "second apply should be a no-op");
}

#[tokio::test]
async fn dry_run_changes_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let path = write_sshd(tmp.path(), "Port 22\n").await;
    let runner = Arc::new(MockRunner::default());
    let ctx = Context::with_parts(config(), tmp.path().to_path_buf(), runner.clone());

    let report = SshModule.apply(&ctx, true).await.unwrap();
    assert!(report.dry_run);
    assert!(!report.skipped.is_empty());

    // File untouched, nothing executed.
    let after = tokio::fs::read_to_string(&path).await.unwrap();
    assert_eq!(after, "Port 22\n");
    assert!(runner.calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn rollback_restores_snapshot() {
    let tmp = tempfile::tempdir().unwrap();
    let original = "Port 22\nPermitRootLogin yes\n";
    let path = write_sshd(tmp.path(), original).await;
    let runner = Arc::new(MockRunner::default());
    let ctx = Context::with_parts(config(), tmp.path().to_path_buf(), runner.clone());
    let module = SshModule;

    module.apply(&ctx, false).await.unwrap();
    module.rollback(&ctx).await.unwrap();

    let restored = tokio::fs::read_to_string(&path).await.unwrap();
    assert_eq!(restored, original);
    // Snapshot consumed.
    assert!(!tmp.path().join("etc/ssh/sshd_config.vpsguard.bak").exists());
}
