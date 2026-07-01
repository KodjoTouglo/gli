//! Tailscale module: install the client and join the tailnet.
//!
//! Installs Tailscale via the official cross-distro script, enables the daemon,
//! and runs `tailscale up` (optionally with a pre-auth key and Tailscale SSH).
//! Opt-in, disabled by default. The auth key is a secret and is never printed
//! in plans or reports.

use async_trait::async_trait;

use vpsguard_core::{Category, Change, Context, Module, Report, Result, Status};

const SERVICE: &str = "tailscaled";
const INSTALL: &str = "curl -fsSL https://tailscale.com/install.sh | sh";

/// Tailscale module.
#[derive(Debug, Default)]
pub struct TailscaleModule;

#[async_trait]
impl Module for TailscaleModule {
    fn name(&self) -> &str {
        "tailscale"
    }

    fn summary(&self) -> &str {
        "Install Tailscale and join the tailnet"
    }

    fn category(&self) -> Category {
        Category::Network
    }

    async fn check(&self, ctx: &Context) -> Result<Status> {
        if !ctx.config.tailscale.enabled {
            return Ok(Status::not_applicable("tailscale disabled in config"));
        }
        Ok(drift_to_status(self_drift(ctx).await))
    }

    async fn plan(&self, ctx: &Context) -> Result<Vec<Change>> {
        if !ctx.config.tailscale.enabled {
            return Ok(Vec::new());
        }
        Ok(self_drift(ctx)
            .await
            .into_iter()
            .map(Change::command)
            .collect())
    }

    async fn apply(&self, ctx: &Context, dry_run: bool) -> Result<Report> {
        let mut report = Report::new("tailscale", dry_run);
        if !ctx.config.tailscale.enabled {
            return Ok(report);
        }
        let drift = self_drift(ctx).await;
        if drift.is_empty() {
            return Ok(report);
        }
        if dry_run {
            report.skipped = drift.into_iter().map(Change::command).collect();
            return Ok(report);
        }

        if !installed(ctx).await {
            ctx.runner().run_checked("sh", &["-c", INSTALL]).await?;
            report.applied.push(Change::command("install tailscale"));
        }
        ctx.runner()
            .run_checked("systemctl", &["enable", "--now", SERVICE])
            .await?;

        let args = up_args(&ctx.config.tailscale);
        let argv: Vec<&str> = args.iter().map(String::as_str).collect();
        ctx.runner().run_checked("tailscale", &argv).await?;
        report.applied.push(Change::command("tailscale up"));
        Ok(report)
    }

    async fn rollback(&self, ctx: &Context) -> Result<()> {
        let _ = ctx.runner().run("tailscale", &["down"]).await;
        Ok(())
    }

    async fn uninstall(&self, ctx: &Context, purge: bool) -> Result<Report> {
        let mut report = Report::new("tailscale", false);
        let _ = ctx.runner().run("tailscale", &["down"]).await;
        crate::common::disable_service(ctx, SERVICE).await;
        crate::common::remove_pkg(ctx, "tailscale", purge).await;
        report.applied.push(Change::command("remove tailscale"));
        if purge {
            let _ = ctx.runner().run("rm", &["-rf", "/var/lib/tailscale"]).await;
            report
                .applied
                .push(Change::command("purge tailscale state"));
        }
        Ok(report)
    }
}

async fn self_drift(ctx: &Context) -> Vec<String> {
    let mut drift = Vec::new();
    if !installed(ctx).await {
        drift.push("install tailscale".into());
    }
    if !up(ctx).await {
        drift.push("tailscale up".into());
    }
    drift
}

// ---------------------------------------------------------------------------
// Pure logic (no IO), unit-tested below.
// ---------------------------------------------------------------------------

/// Build the `tailscale up` argument list. The auth key, if any, is included
/// but callers must not log the result.
fn up_args(cfg: &vpsguard_core::TailscaleConfig) -> Vec<String> {
    let mut args = vec!["up".to_string()];
    if let Some(key) = &cfg.auth_key {
        args.push(format!("--authkey={key}"));
    }
    if cfg.ssh {
        args.push("--ssh".to_string());
    }
    args
}

// ---------------------------------------------------------------------------
// IO
// ---------------------------------------------------------------------------

fn drift_to_status(drift: Vec<String>) -> Status {
    if drift.is_empty() {
        Status::compliant()
    } else {
        Status::drift(drift.join("; "))
    }
}

async fn installed(ctx: &Context) -> bool {
    ctx.runner()
        .run("tailscale", &["version"])
        .await
        .map(|o| o.success())
        .unwrap_or(false)
}

/// True when the node is up (logged in and connected).
async fn up(ctx: &Context) -> bool {
    match ctx.runner().run("tailscale", &["status"]).await {
        Ok(o) => o.success() && !o.stdout.contains("Logged out"),
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vpsguard_core::TailscaleConfig;

    #[test]
    fn up_args_minimal() {
        let cfg = TailscaleConfig {
            enabled: true,
            auth_key: None,
            ssh: false,
        };
        assert_eq!(up_args(&cfg), vec!["up"]);
    }

    #[test]
    fn up_args_with_key_and_ssh() {
        let cfg = TailscaleConfig {
            enabled: true,
            auth_key: Some("tskey-abc".into()),
            ssh: true,
        };
        let args = up_args(&cfg);
        assert!(args.contains(&"--authkey=tskey-abc".to_string()));
        assert!(args.contains(&"--ssh".to_string()));
    }
}
