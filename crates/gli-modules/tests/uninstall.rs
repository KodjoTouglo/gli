//! Uninstall behaviour: package/service removal, and data kept unless --purge.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use gli_core::{
    CommandRunner, Config, Context, DistroFamily, DockerConfig, Module, Output, Platform, Result,
};
use gli_modules::{DockerModule, FirewallModule};

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

fn ctx(cfg: Config, family: DistroFamily) -> (Context, Arc<MockRunner>) {
    let runner = Arc::new(MockRunner::default());
    let ctx = Context::with_parts(cfg, std::path::PathBuf::from("/"), runner.clone())
        .with_platform(Platform {
            family,
            id: String::new(),
        });
    (ctx, runner)
}

fn docker_cfg() -> Config {
    Config {
        docker: DockerConfig {
            enabled: true,
            users: vec!["deploy".into()],
        },
        ..Config::default()
    }
}

#[tokio::test]
async fn docker_uninstall_removes_package_keeps_data_by_default() {
    let (ctx, runner) = ctx(docker_cfg(), DistroFamily::Debian);
    DockerModule.uninstall(&ctx, false).await.unwrap();

    let calls = runner.calls.lock().unwrap().clone();
    assert!(calls.iter().any(|c| c == "gpasswd -d deploy docker"));
    assert!(calls.iter().any(|c| c == "systemctl disable --now docker"));
    assert!(calls.iter().any(|c| c == "apt-get remove -y docker.io"));
    // No data purge without --purge.
    assert!(calls.iter().all(|c| !c.contains("/var/lib/docker")));
}

#[tokio::test]
async fn docker_uninstall_purge_deletes_data() {
    let (ctx, runner) = ctx(docker_cfg(), DistroFamily::Debian);
    DockerModule.uninstall(&ctx, true).await.unwrap();

    let calls = runner.calls.lock().unwrap().clone();
    assert!(calls.iter().any(|c| c == "apt-get purge -y docker.io"));
    assert!(calls.iter().any(|c| c == "rm -rf /var/lib/docker"));
}

#[tokio::test]
async fn rhel_uses_dnf_remove() {
    let (ctx, runner) = ctx(docker_cfg(), DistroFamily::Rhel);
    DockerModule.uninstall(&ctx, false).await.unwrap();
    assert!(runner
        .calls
        .lock()
        .unwrap()
        .iter()
        .any(|c| c == "dnf remove -y docker"));
}

#[tokio::test]
async fn firewall_uninstall_deletes_table() {
    let (ctx, runner) = ctx(Config::default(), DistroFamily::Debian);
    FirewallModule.uninstall(&ctx, false).await.unwrap();
    assert!(runner
        .calls
        .lock()
        .unwrap()
        .iter()
        .any(|c| c == "nft delete table inet gli"));
}
