//! Exercise UsersModule against a tempdir root with a mocked runner.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use gli_core::{CommandRunner, Config, Context, Module, Output, Result, State, UserConfig};
use gli_modules::UsersModule;

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
    let mut users = std::collections::BTreeMap::new();
    users.insert(
        "deploy".to_string(),
        UserConfig {
            sudo: true,
            ssh_keys: vec!["ssh-ed25519 AAAAkey deploy@host".into()],
        },
    );
    Config {
        users,
        ..Config::default()
    }
}

fn ctx(root: &std::path::Path) -> (Context, Arc<MockRunner>) {
    let runner = Arc::new(MockRunner::default());
    (
        Context::with_parts(config(), root.to_path_buf(), runner.clone()),
        runner,
    )
}

async fn seed(root: &std::path::Path, rel: &str, body: &str) {
    let p = root.join(rel);
    tokio::fs::create_dir_all(p.parent().unwrap())
        .await
        .unwrap();
    tokio::fs::write(&p, body).await.unwrap();
}

#[tokio::test]
async fn creates_user_grants_sudo_installs_keys() {
    let tmp = tempfile::tempdir().unwrap();
    let (ctx, runner) = ctx(tmp.path());

    let report = UsersModule.apply(&ctx, false).await.unwrap();
    assert!(!report.is_noop());

    let ak = tmp.path().join("home/deploy/.ssh/authorized_keys");
    assert!(tokio::fs::read_to_string(&ak)
        .await
        .unwrap()
        .contains("ssh-ed25519 AAAAkey deploy@host"));

    let sudoers = tmp.path().join("etc/sudoers.d/gli-deploy");
    assert_eq!(
        tokio::fs::read_to_string(&sudoers).await.unwrap(),
        "deploy ALL=(ALL:ALL) ALL\n"
    );

    let calls = runner.calls.lock().unwrap().clone();
    assert!(calls.iter().any(|c| c.starts_with("useradd")));
    assert!(calls.iter().any(|c| c.starts_with("visudo -cf")));
    assert!(calls
        .iter()
        .any(|c| c.starts_with("chown -R deploy:deploy")));
}

#[tokio::test]
async fn dry_run_writes_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let (ctx, runner) = ctx(tmp.path());

    let report = UsersModule.apply(&ctx, true).await.unwrap();
    assert!(report.dry_run);
    assert!(!report.skipped.is_empty());
    assert!(!tmp.path().join("etc/sudoers.d/gli-deploy").exists());
    assert!(runner.calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn compliant_when_already_set_up() {
    let tmp = tempfile::tempdir().unwrap();
    seed(
        tmp.path(),
        "etc/passwd",
        "root:x:0:0::/root:/bin/bash\ndeploy:x:1000:1000::/home/deploy:/bin/bash\n",
    )
    .await;
    seed(
        tmp.path(),
        "home/deploy/.ssh/authorized_keys",
        "ssh-ed25519 AAAAkey deploy@host\n",
    )
    .await;
    seed(
        tmp.path(),
        "etc/sudoers.d/gli-deploy",
        "deploy ALL=(ALL:ALL) ALL\n",
    )
    .await;

    let (ctx, runner) = ctx(tmp.path());
    let status = UsersModule.check(&ctx).await.unwrap();
    assert_eq!(status.state, State::Compliant);

    let report = UsersModule.apply(&ctx, false).await.unwrap();
    assert!(report.is_noop());
    assert!(runner.calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn rollback_removes_sudoers() {
    let tmp = tempfile::tempdir().unwrap();
    seed(
        tmp.path(),
        "etc/sudoers.d/gli-deploy",
        "deploy ALL=(ALL:ALL) ALL\n",
    )
    .await;
    let (ctx, _) = ctx(tmp.path());

    UsersModule.rollback(&ctx).await.unwrap();
    assert!(!tmp.path().join("etc/sudoers.d/gli-deploy").exists());
}
