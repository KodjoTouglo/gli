//! Exercise MonitoringModule with a mocked runner.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use gli_core::{
    CommandRunner, Config, Context, DistroFamily, Module, MonitoringBackend, MonitoringConfig,
    Output, Platform, Result, State,
};
use gli_modules::MonitoringModule;

/// Reports the agent absent (command -v fails) and the service disabled.
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
        let absent = command == "sh"
            && args.first() == Some(&"-c")
            && args.get(1).is_some_and(|a| a.starts_with("command -v"));
        let disabled = command == "systemctl" && args.first() == Some(&"is-enabled");
        Ok(Output {
            code: if absent || disabled { 1 } else { 0 },
            stdout: if disabled {
                "disabled".into()
            } else {
                String::new()
            },
            stderr: String::new(),
        })
    }
}

fn config(backend: MonitoringBackend) -> Config {
    Config {
        monitoring: MonitoringConfig {
            enabled: true,
            backend,
        },
        ..Config::default()
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

#[tokio::test]
async fn netdata_installs_via_kickstart_and_enables() {
    let (ctx, runner) = ctx(config(MonitoringBackend::Netdata), DistroFamily::Debian);
    let report = MonitoringModule.apply(&ctx, false).await.unwrap();
    assert!(!report.is_noop());

    let calls = runner.calls.lock().unwrap().clone();
    assert!(calls.iter().any(|c| c.contains("kickstart")));
    assert!(calls.iter().any(|c| c == "systemctl enable --now netdata"));
}

#[tokio::test]
async fn node_exporter_installs_debian_package() {
    let (ctx, runner) = ctx(
        config(MonitoringBackend::NodeExporter),
        DistroFamily::Debian,
    );
    MonitoringModule.apply(&ctx, false).await.unwrap();
    let calls = runner.calls.lock().unwrap().clone();
    assert!(calls
        .iter()
        .any(|c| c == "apt-get install -y prometheus-node-exporter"));
    assert!(calls
        .iter()
        .any(|c| c == "systemctl enable --now prometheus-node-exporter"));
}

#[tokio::test]
async fn disabled_is_not_applicable() {
    let mut cfg = config(MonitoringBackend::Netdata);
    cfg.monitoring.enabled = false;
    let (ctx, _) = ctx(cfg, DistroFamily::Debian);
    assert_eq!(
        MonitoringModule.check(&ctx).await.unwrap().state,
        State::NotApplicable
    );
}

#[tokio::test]
async fn node_exporter_unsupported_distro_not_applicable() {
    let (ctx, _) = ctx(config(MonitoringBackend::NodeExporter), DistroFamily::Suse);
    assert_eq!(
        MonitoringModule.check(&ctx).await.unwrap().state,
        State::NotApplicable
    );
}
