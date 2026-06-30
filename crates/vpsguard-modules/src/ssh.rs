//! SSH hardening module, reference implementation of the `Module` trait.
//!
//! Idempotently manages Port, PermitRootLogin, PasswordAuthentication and the
//! modern Ciphers/KexAlgorithms/MACs set. A rewritten config is validated with
//! `sshd -t` before it replaces the live file, the daemon is restarted only on
//! success, and the previous file is snapshotted for rollback. The CLI lockout
//! guard builds the timed auto-rollback on top of this snapshot/rollback.

use async_trait::async_trait;

use std::path::Path;

use vpsguard_core::{Category, Change, Context, Error, Module, Report, Result, SshConfig, Status};

use crate::common::with_suffix;

const SSHD_CONFIG: &str = "/etc/ssh/sshd_config";
/// Suffix for the pre-apply snapshot used by `rollback`.
const BACKUP_SUFFIX: &str = ".vpsguard.bak";
/// Suffix for the staged file we validate before swapping it in.
const STAGED_SUFFIX: &str = ".vpsguard.new";

// Modern, conservative crypto. Mirrors common CIS / Mozilla "modern" guidance.
const CIPHERS: &str = "chacha20-poly1305@openssh.com,aes256-gcm@openssh.com,aes128-gcm@openssh.com";
const KEX_ALGORITHMS: &str =
    "curve25519-sha256,curve25519-sha256@libssh.org,diffie-hellman-group16-sha512";
const MACS: &str =
    "hmac-sha2-512-etm@openssh.com,hmac-sha2-256-etm@openssh.com,umac-128-etm@openssh.com";

/// SSH daemon hardening module.
#[derive(Debug, Default)]
pub struct SshModule;

#[async_trait]
impl Module for SshModule {
    fn name(&self) -> &str {
        "ssh"
    }

    fn summary(&self) -> &str {
        "Harden sshd: custom port, no root/password login, modern crypto"
    }

    fn category(&self) -> Category {
        Category::Security
    }

    fn lockout_risk(&self) -> bool {
        true
    }

    async fn check(&self, ctx: &Context) -> Result<Status> {
        let Some(content) = ctx.read(SSHD_CONFIG).await? else {
            return Ok(Status::not_applicable(
                "sshd_config not found (sshd not installed?)",
            ));
        };

        let drift = diff(&content, &ctx.config.ssh);
        if drift.is_empty() {
            Ok(Status::compliant())
        } else {
            let detail = drift
                .iter()
                .map(|c| c.summary.clone())
                .collect::<Vec<_>>()
                .join("; ");
            Ok(Status::drift(detail))
        }
    }

    async fn plan(&self, ctx: &Context) -> Result<Vec<Change>> {
        let content = ctx.read_or_empty(SSHD_CONFIG).await?;
        Ok(diff(&content, &ctx.config.ssh))
    }

    async fn apply(&self, ctx: &Context, dry_run: bool) -> Result<Report> {
        let Some(content) = ctx.read(SSHD_CONFIG).await? else {
            return Err(Error::Module {
                module: "ssh".into(),
                message: "sshd_config not found; nothing to harden".into(),
            });
        };

        let changes = diff(&content, &ctx.config.ssh);
        let mut report = Report::new("ssh", dry_run);

        if changes.is_empty() {
            return Ok(report); // already compliant, true no-op
        }

        if dry_run {
            report.skipped = changes;
            return Ok(report);
        }

        let rendered = render(&content, &desired_directives(&ctx.config.ssh));

        // 1. Snapshot the current file for rollback.
        let backup = with_suffix(Path::new(SSHD_CONFIG), BACKUP_SUFFIX);
        ctx.write(&backup, &content).await?;

        // 2. Stage + validate before touching the live file.
        let staged = with_suffix(Path::new(SSHD_CONFIG), STAGED_SUFFIX);
        ctx.write(&staged, &rendered).await?;
        let staged_str = ctx.path(&staged).to_string_lossy().into_owned();
        let validation = ctx.runner().run("sshd", &["-t", "-f", &staged_str]).await?;
        if !validation.success() {
            let _ = ctx.remove(&staged).await;
            return Err(Error::Safety(format!(
                "candidate sshd_config rejected by `sshd -t`: {}",
                validation.stderr.trim()
            )));
        }

        // 3. Swap staged file into place, then restart the daemon.
        ctx.rename(&staged, Path::new(SSHD_CONFIG)).await?;
        let service = ctx.platform().ssh_service();
        ctx.runner()
            .run_checked("systemctl", &["restart", service])
            .await?;

        report.applied = changes;
        report
            .applied
            .push(Change::command(format!("systemctl restart {service}")));
        Ok(report)
    }

    async fn rollback(&self, ctx: &Context) -> Result<()> {
        let backup = with_suffix(Path::new(SSHD_CONFIG), BACKUP_SUFFIX);
        let Some(saved) = ctx.read(&backup).await? else {
            return Err(Error::Module {
                module: "ssh".into(),
                message: "no snapshot to roll back to".into(),
            });
        };

        ctx.write(SSHD_CONFIG, &saved).await?;
        ctx.runner()
            .run_checked("systemctl", &["restart", ctx.platform().ssh_service()])
            .await?;
        let _ = ctx.remove(&backup).await;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Pure logic (no IO), unit-tested below.
// ---------------------------------------------------------------------------

/// The directives vpsguard manages, in the order they should appear.
fn desired_directives(cfg: &SshConfig) -> Vec<(&'static str, String)> {
    let mut d = vec![
        ("Port", cfg.port.to_string()),
        (
            "PermitRootLogin",
            if cfg.permit_root_login { "yes" } else { "no" }.to_string(),
        ),
        (
            "PasswordAuthentication",
            if cfg.password_auth { "yes" } else { "no" }.to_string(),
        ),
    ];
    if cfg.modern_ciphers {
        d.push(("Ciphers", CIPHERS.to_string()));
        d.push(("KexAlgorithms", KEX_ALGORITHMS.to_string()));
        d.push(("MACs", MACS.to_string()));
    }
    d
}

/// Compute the changes needed to bring `content` in line with `cfg`.
fn diff(content: &str, cfg: &SshConfig) -> Vec<Change> {
    desired_directives(cfg)
        .into_iter()
        .filter_map(|(key, want)| match active_value(content, key) {
            Some(cur) if values_equal(&cur, &want) => None,
            Some(cur) => Some(Change::modify(format!("{key}: {cur} -> {want}"), cur, want)),
            None => Some(Change::modify(
                format!("{key}: (unset) -> {want}"),
                "(unset)",
                want,
            )),
        })
        .collect()
}

/// Apply `desired` to `content`, returning the new file body.
///
/// Idempotent: the first active occurrence of each key is rewritten in place;
/// any further active duplicates are commented out; missing keys are appended.
/// Commented templates and unrelated lines are preserved.
fn render(content: &str, desired: &[(&str, String)]) -> String {
    let mut lines: Vec<String> = content.lines().map(str::to_string).collect();
    let had_trailing_newline = content.ends_with('\n') || content.is_empty();

    for (key, val) in desired {
        let target = format!("{key} {val}");
        let mut placed = false;
        for line in lines.iter_mut() {
            if is_active_directive(line, key) {
                if placed {
                    *line = format!("# {line}  # superseded by vpsguard");
                } else {
                    *line = target.clone();
                    placed = true;
                }
            }
        }
        if !placed {
            lines.push(target);
        }
    }

    let mut out = lines.join("\n");
    if had_trailing_newline {
        out.push('\n');
    }
    out
}

/// First active (uncommented) value for `key`, per sshd "first match wins".
fn active_value(content: &str, key: &str) -> Option<String> {
    content
        .lines()
        .find(|l| is_active_directive(l, key))
        .map(directive_value)
}

/// True if `line` is an uncommented directive whose keyword is `key`.
fn is_active_directive(line: &str, key: &str) -> bool {
    let t = line.trim_start();
    if t.is_empty() || t.starts_with('#') {
        return false;
    }
    t.split_whitespace()
        .next()
        .is_some_and(|tok| tok.eq_ignore_ascii_case(key))
}

/// Everything after the keyword on a directive line, trimmed.
fn directive_value(line: &str) -> String {
    let t = line.trim();
    match t.split_once(char::is_whitespace) {
        Some((_, rest)) => rest.trim().to_string(),
        None => String::new(),
    }
}

/// Compare directive values: case-insensitive (covers `yes`/`no`, algo names),
/// which is how `sshd` itself treats keywords and booleans.
fn values_equal(a: &str, b: &str) -> bool {
    a.eq_ignore_ascii_case(b)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> SshConfig {
        SshConfig {
            port: 2222,
            permit_root_login: false,
            password_auth: false,
            modern_ciphers: false,
        }
    }

    #[test]
    fn appends_missing_directives() {
        let out = render("# empty config\n", &desired_directives(&cfg()));
        assert_eq!(active_value(&out, "Port").as_deref(), Some("2222"));
        assert_eq!(active_value(&out, "PermitRootLogin").as_deref(), Some("no"));
        assert_eq!(
            active_value(&out, "PasswordAuthentication").as_deref(),
            Some("no")
        );
    }

    #[test]
    fn replaces_existing_active_directive() {
        let input = "Port 22\nPermitRootLogin yes\nPasswordAuthentication yes\n";
        let out = render(input, &desired_directives(&cfg()));
        assert_eq!(active_value(&out, "Port").as_deref(), Some("2222"));
        assert_eq!(active_value(&out, "PermitRootLogin").as_deref(), Some("no"));
        // Replaced in place, not duplicated.
        assert_eq!(out.matches("Port ").count(), 1);
    }

    #[test]
    fn ignores_commented_directives_and_appends() {
        let input = "#Port 22\n#PermitRootLogin yes\n";
        let out = render(input, &desired_directives(&cfg()));
        // Original comments preserved.
        assert!(out.contains("#Port 22"));
        // Active directive appended.
        assert_eq!(active_value(&out, "Port").as_deref(), Some("2222"));
    }

    #[test]
    fn comments_out_duplicate_actives() {
        let input = "Port 22\nPort 80\n";
        let out = render(input, &desired_directives(&cfg()));
        assert_eq!(active_value(&out, "Port").as_deref(), Some("2222"));
        assert!(out.contains("superseded by vpsguard"));
        // Only one active Port line remains.
        let actives = out
            .lines()
            .filter(|l| is_active_directive(l, "Port"))
            .count();
        assert_eq!(actives, 1);
    }

    #[test]
    fn render_is_idempotent() {
        let input = "Port 22\nPermitRootLogin yes\n";
        let desired = desired_directives(&cfg());
        let once = render(input, &desired);
        let twice = render(&once, &desired);
        assert_eq!(once, twice);
    }

    #[test]
    fn keyword_match_is_case_insensitive() {
        let input = "port 22\n";
        let out = render(input, &desired_directives(&cfg()));
        assert_eq!(active_value(&out, "Port").as_deref(), Some("2222"));
        let actives = out
            .lines()
            .filter(|l| is_active_directive(l, "Port"))
            .count();
        assert_eq!(actives, 1); // replaced in place, not appended
    }

    #[test]
    fn diff_reports_only_drifting_keys() {
        // Port already correct; root login wrong.
        let input = "Port 2222\nPermitRootLogin yes\nPasswordAuthentication no\n";
        let changes = diff(input, &cfg());
        assert_eq!(changes.len(), 1);
        assert!(changes[0].summary.starts_with("PermitRootLogin"));
    }

    #[test]
    fn ssh_is_lockout_risky() {
        assert!(SshModule.lockout_risk());
    }

    #[test]
    fn diff_empty_when_compliant() {
        let input = "Port 2222\nPermitRootLogin no\nPasswordAuthentication no\n";
        assert!(diff(input, &cfg()).is_empty());
    }

    #[test]
    fn modern_ciphers_add_three_directives() {
        let c = SshConfig {
            modern_ciphers: true,
            ..cfg()
        };
        let out = render("", &desired_directives(&c));
        assert_eq!(active_value(&out, "Ciphers").as_deref(), Some(CIPHERS));
        assert_eq!(
            active_value(&out, "KexAlgorithms").as_deref(),
            Some(KEX_ALGORITHMS)
        );
        assert_eq!(active_value(&out, "MACs").as_deref(), Some(MACS));
    }
}
