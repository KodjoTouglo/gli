//! Small helpers shared by modules.

use std::path::{Path, PathBuf};

use vpsguard_core::{Context, DistroFamily};

/// Append `suffix` to a path's filename (e.g. `.bak`).
pub(crate) fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut s = path.as_os_str().to_os_string();
    s.push(suffix);
    PathBuf::from(s)
}

/// Remove a package with the host package manager (best-effort). `purge` also
/// removes config files where the manager supports it.
pub(crate) async fn remove_pkg(ctx: &Context, pkg: &str, purge: bool) {
    let cmd: Option<(&str, Vec<&str>)> = match ctx.platform().family {
        DistroFamily::Debian => Some((
            "apt-get",
            vec![if purge { "purge" } else { "remove" }, "-y", pkg],
        )),
        DistroFamily::Rhel => Some(("dnf", vec!["remove", "-y", pkg])),
        DistroFamily::Arch => Some(("pacman", vec!["-Rns", "--noconfirm", pkg])),
        DistroFamily::Suse => Some(("zypper", vec!["remove", "-y", pkg])),
        DistroFamily::Unknown => None,
    };
    if let Some((c, args)) = cmd {
        let _ = ctx.runner().run(c, &args).await;
    }
}

/// Stop and disable a systemd service (best-effort).
pub(crate) async fn disable_service(ctx: &Context, service: &str) {
    let _ = ctx
        .runner()
        .run("systemctl", &["disable", "--now", service])
        .await;
}
