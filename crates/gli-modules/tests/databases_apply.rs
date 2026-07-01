//! Exercise PostgresModule and RedisModule with a mocked runner.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use gli_core::{
    CommandRunner, Config, Context, DistroFamily, Module, Output, Platform, PostgresConfig,
    RedisConfig, Result, State,
};
use gli_modules::{PostgresModule, RedisModule};

/// Reports packages/services absent so apply takes the install path.
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
            ("psql", Some("--version")) => (1, ""),
            ("redis-server", Some("--version")) => (1, ""),
            ("systemctl", Some("is-enabled")) => (1, "disabled"),
            ("sudo", _) => (0, ""), // psql -tAc returns empty -> db missing
            _ => (0, ""),
        };
        Ok(Output {
            code,
            stdout: stdout.into(),
            stderr: String::new(),
        })
    }
}

fn ctx(family: DistroFamily, cfg: Config) -> (Context, Arc<MockRunner>) {
    let runner = Arc::new(MockRunner::default());
    let ctx = Context::with_parts(cfg, std::path::PathBuf::from("/"), runner.clone())
        .with_platform(Platform {
            family,
            id: String::new(),
        });
    (ctx, runner)
}

#[tokio::test]
async fn postgres_installs_enables_and_creates_db() {
    let cfg = Config {
        postgres: PostgresConfig {
            enabled: true,
            databases: vec!["myapp".into()],
        },
        ..Config::default()
    };
    let (ctx, runner) = ctx(DistroFamily::Debian, cfg);

    let report = PostgresModule.apply(&ctx, false).await.unwrap();
    assert!(!report.is_noop());

    let calls = runner.calls.lock().unwrap().clone();
    assert!(calls.iter().any(|c| c == "apt-get install -y postgresql"));
    assert!(calls
        .iter()
        .any(|c| c == "systemctl enable --now postgresql"));
    assert!(calls.iter().any(|c| c == "sudo -u postgres createdb myapp"));
}

#[tokio::test]
async fn postgres_disabled_is_not_applicable() {
    let (ctx, _) = ctx(DistroFamily::Debian, Config::default());
    assert_eq!(
        PostgresModule.check(&ctx).await.unwrap().state,
        State::NotApplicable
    );
}

#[tokio::test]
async fn redis_installs_and_enables() {
    let cfg = Config {
        redis: RedisConfig { enabled: true },
        ..Config::default()
    };
    let (ctx, runner) = ctx(DistroFamily::Debian, cfg);

    RedisModule.apply(&ctx, false).await.unwrap();
    let calls = runner.calls.lock().unwrap().clone();
    assert!(calls.iter().any(|c| c == "apt-get install -y redis-server"));
    assert!(calls
        .iter()
        .any(|c| c == "systemctl enable --now redis-server"));
}

#[tokio::test]
async fn redis_uses_distro_package_names() {
    let cfg = Config {
        redis: RedisConfig { enabled: true },
        ..Config::default()
    };
    let (ctx, runner) = ctx(DistroFamily::Rhel, cfg);
    RedisModule.apply(&ctx, false).await.unwrap();
    let calls = runner.calls.lock().unwrap().clone();
    assert!(calls.iter().any(|c| c == "dnf install -y redis"));
    assert!(calls.iter().any(|c| c == "systemctl enable --now redis"));
}

#[tokio::test]
async fn redis_dry_run_writes_nothing() {
    let cfg = Config {
        redis: RedisConfig { enabled: true },
        ..Config::default()
    };
    let (ctx, runner) = ctx(DistroFamily::Debian, cfg);
    let report = RedisModule.apply(&ctx, true).await.unwrap();
    assert!(report.dry_run);
    assert!(runner
        .calls
        .lock()
        .unwrap()
        .iter()
        .all(|c| !c.contains("install -y")));
}
