//! Monitoring module: install a metrics agent.
//!
//! Two backends, chosen in config. `netdata` is an all-in-one agent with a
//! built-in web dashboard, installed via the official cross-distro kickstart
//! (one command, every distro). `node_exporter` exposes Prometheus metrics for
//! a central Prometheus to scrape, installed from the distro package. Opt-in,
//! disabled by default.

use async_trait::async_trait;

use gli_core::{
    Category, Change, Context, DistroFamily, Module, MonitoringBackend, Platform, Report, Result,
    Status,
};

const NETDATA_KICKSTART: &str =
    "curl -Ss https://get.netdata.cloud/kickstart.sh | sh -s -- --non-interactive --stable-channel";

/// Monitoring module.
#[derive(Debug, Default)]
pub struct MonitoringModule;

#[async_trait]
impl Module for MonitoringModule {
    fn name(&self) -> &str {
        "monitoring"
    }

    fn summary(&self) -> &str {
        "Install a metrics agent (netdata dashboard or node_exporter)"
    }

    fn category(&self) -> Category {
        Category::System
    }

    async fn check(&self, ctx: &Context) -> Result<Status> {
        if !ctx.config.monitoring.enabled {
            return Ok(Status::not_applicable("monitoring disabled in config"));
        }
        let Some(ops) = ops(ctx.config.monitoring.backend, ctx.platform()) else {
            return Ok(Status::not_applicable(
                "selected monitoring backend not available for this distro",
            ));
        };
        Ok(drift_to_status(self_drift(ctx, &ops).await))
    }

    async fn plan(&self, ctx: &Context) -> Result<Vec<Change>> {
        let Some(ops) = enabled_ops(ctx) else {
            return Ok(Vec::new());
        };
        Ok(self_drift(ctx, &ops)
            .await
            .into_iter()
            .map(Change::command)
            .collect())
    }

    async fn apply(&self, ctx: &Context, dry_run: bool) -> Result<Report> {
        let mut report = Report::new("monitoring", dry_run);
        let Some(ops) = enabled_ops(ctx) else {
            return Ok(report);
        };

        let drift = self_drift(ctx, &ops).await;
        if drift.is_empty() {
            return Ok(report);
        }
        if dry_run {
            report.skipped = drift.into_iter().map(Change::command).collect();
            return Ok(report);
        }

        if !installed(ctx, &ops).await {
            for (cmd, args) in &ops.install {
                ctx.runner().run_checked(cmd, args).await?;
            }
            report
                .applied
                .push(Change::command("install monitoring agent"));
        }
        ctx.runner()
            .run_checked("systemctl", &["enable", "--now", ops.service])
            .await?;
        report
            .applied
            .push(Change::command(format!("enable {}", ops.service)));
        Ok(report)
    }

    async fn rollback(&self, ctx: &Context) -> Result<()> {
        if let Some(ops) = enabled_ops(ctx) {
            crate::common::disable_service(ctx, ops.service).await;
        }
        Ok(())
    }

    async fn uninstall(&self, ctx: &Context, purge: bool) -> Result<Report> {
        let mut report = Report::new("monitoring", false);
        let Some(ops) = ops(ctx.config.monitoring.backend, ctx.platform()) else {
            return Ok(report);
        };
        crate::common::disable_service(ctx, ops.service).await;
        match ops.pkg {
            Some(pkg) => crate::common::remove_pkg(ctx, pkg, purge).await,
            None => {
                // netdata ships its own uninstaller.
                let _ = ctx
                    .runner()
                    .run(
                        "sh",
                        &["-c", "netdata-uninstaller.sh --yes --force || true"],
                    )
                    .await;
            }
        }
        report
            .applied
            .push(Change::command("remove monitoring agent"));
        if purge {
            let _ = ctx
                .runner()
                .run("rm", &["-rf", "/var/lib/netdata", "/etc/netdata"])
                .await;
            report
                .applied
                .push(Change::command("purge monitoring data"));
        }
        Ok(report)
    }
}

fn enabled_ops(ctx: &Context) -> Option<Ops> {
    if !ctx.config.monitoring.enabled {
        return None;
    }
    ops(ctx.config.monitoring.backend, ctx.platform())
}

async fn self_drift(ctx: &Context, ops: &Ops) -> Vec<String> {
    let mut drift = Vec::new();
    if !installed(ctx, ops).await {
        drift.push("install monitoring agent".into());
    }
    if !service_enabled(ctx, ops.service).await {
        drift.push(format!("enable {}", ops.service));
    }
    drift
}

// ---------------------------------------------------------------------------
// Pure logic (no IO), unit-tested below.
// ---------------------------------------------------------------------------

/// How to install and manage a backend on this host.
struct Ops {
    service: &'static str,
    probe: &'static str,
    install: Vec<(&'static str, Vec<&'static str>)>,
    /// Package name for uninstall; None means use the backend's own uninstaller.
    pkg: Option<&'static str>,
}

fn ops(backend: MonitoringBackend, p: &Platform) -> Option<Ops> {
    match backend {
        MonitoringBackend::Netdata => Some(Ops {
            service: "netdata",
            probe: "netdata",
            install: vec![("sh", vec!["-c", NETDATA_KICKSTART])],
            pkg: None,
        }),
        MonitoringBackend::NodeExporter => match p.family {
            DistroFamily::Debian => Some(Ops {
                service: "prometheus-node-exporter",
                probe: "prometheus-node-exporter",
                install: vec![("apt-get", vec!["install", "-y", "prometheus-node-exporter"])],
                pkg: Some("prometheus-node-exporter"),
            }),
            DistroFamily::Rhel => Some(Ops {
                service: "node_exporter",
                probe: "node_exporter",
                install: vec![("dnf", vec!["install", "-y", "node_exporter"])],
                pkg: Some("node_exporter"),
            }),
            DistroFamily::Arch => Some(Ops {
                service: "prometheus-node-exporter",
                probe: "node_exporter",
                install: vec![(
                    "pacman",
                    vec!["-S", "--noconfirm", "prometheus-node-exporter"],
                )],
                pkg: Some("prometheus-node-exporter"),
            }),
            DistroFamily::Suse | DistroFamily::Unknown => None,
        },
    }
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

async fn installed(ctx: &Context, ops: &Ops) -> bool {
    ctx.runner()
        .run("sh", &["-c", &format!("command -v {}", ops.probe)])
        .await
        .map(|o| o.success())
        .unwrap_or(false)
}

async fn service_enabled(ctx: &Context, service: &str) -> bool {
    ctx.runner()
        .run("systemctl", &["is-enabled", service])
        .await
        .map(|o| o.stdout.trim() == "enabled")
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn platform(family: DistroFamily) -> Platform {
        Platform {
            family,
            id: String::new(),
        }
    }

    #[test]
    fn netdata_uses_kickstart_on_any_distro() {
        let o = ops(MonitoringBackend::Netdata, &platform(DistroFamily::Suse)).unwrap();
        assert_eq!(o.service, "netdata");
        assert!(o.install[0].1.iter().any(|a| a.contains("kickstart")));
        assert!(o.pkg.is_none());
    }

    #[test]
    fn node_exporter_debian_package() {
        let o = ops(
            MonitoringBackend::NodeExporter,
            &platform(DistroFamily::Debian),
        )
        .unwrap();
        assert_eq!(o.service, "prometheus-node-exporter");
        assert_eq!(o.pkg, Some("prometheus-node-exporter"));
    }

    #[test]
    fn node_exporter_unsupported_distro_is_none() {
        assert!(ops(
            MonitoringBackend::NodeExporter,
            &platform(DistroFamily::Unknown)
        )
        .is_none());
    }
}
