//! nftables firewall module.
//!
//! vpsguard owns a dedicated `inet vpsguard` table and reprograms it atomically
//! with `nft -f`. The configured input policy plus allow rules are rendered to a
//! script that always permits loopback, established/related traffic, and the SSH
//! port (lockout guard). A config hash is stored as a rule comment so `check`
//! can tell whether the live table is up to date. The script is validated with
//! `nft -c` before it is applied, and the previous table is snapshotted for
//! rollback.

use async_trait::async_trait;

use vpsguard_core::{
    Category, Change, Context, Error, FirewallConfig, Module, Policy, Report, Result, Status,
};

use crate::common::write;

const TABLE: &str = "vpsguard";
const STAGED: &str = "/etc/vpsguard/firewall.staged.nft";
const BACKUP: &str = "/etc/vpsguard/firewall.backup.nft";

/// nftables firewall module.
#[derive(Debug, Default)]
pub struct FirewallModule;

#[async_trait]
impl Module for FirewallModule {
    fn name(&self) -> &str {
        "firewall"
    }

    fn summary(&self) -> &str {
        "nftables: default-deny input, allow listed ports, SSH protected"
    }

    fn category(&self) -> Category {
        Category::Security
    }

    async fn check(&self, ctx: &Context) -> Result<Status> {
        if !ctx.config.firewall.enabled {
            return Ok(Status::not_applicable("firewall disabled in config"));
        }
        let want = plan_ruleset(ctx)?;
        match nft_list_table(ctx).await {
            Err(_) => Ok(Status::not_applicable("nftables not available")),
            Ok(None) => Ok(Status::drift("firewall table not present")),
            Ok(Some(rules)) if rules.contains(&marker(&want.hash)) => Ok(Status::compliant()),
            Ok(Some(_)) => Ok(Status::drift("firewall rules out of date")),
        }
    }

    async fn plan(&self, ctx: &Context) -> Result<Vec<Change>> {
        Ok(plan_ruleset(ctx)?.changes)
    }

    async fn apply(&self, ctx: &Context, dry_run: bool) -> Result<Report> {
        let mut report = Report::new("firewall", dry_run);
        if !ctx.config.firewall.enabled {
            return Ok(report);
        }

        let want = plan_ruleset(ctx)?;
        let live = nft_list_table(ctx).await.map_err(|_| Error::Module {
            module: "firewall".into(),
            message: "nftables not installed; install the `nftables` package".into(),
        })?;
        if matches!(&live, Some(rules) if rules.contains(&marker(&want.hash))) {
            return Ok(report); // already up to date
        }

        if dry_run {
            report.skipped = want.changes;
            return Ok(report);
        }

        // Snapshot the current table so rollback can restore (or remove) it.
        let backup = ctx.path(BACKUP);
        let backup_body = match live {
            Some(rules) => format!("delete table inet {TABLE}\n{rules}"),
            None => format!("delete table inet {TABLE}\n"),
        };
        write(&backup, &backup_body).await?;

        // Stage, validate, then apply atomically.
        let staged = ctx.path(STAGED);
        write(&staged, &want.script).await?;
        let staged_str = staged.to_string_lossy().into_owned();
        let validation = ctx.runner().run("nft", &["-c", "-f", &staged_str]).await?;
        if !validation.success() {
            let _ = tokio::fs::remove_file(&staged).await;
            return Err(Error::Safety(format!(
                "candidate nft ruleset rejected: {}",
                validation.stderr.trim()
            )));
        }
        ctx.runner()
            .run_checked("nft", &["-f", &staged_str])
            .await?;

        report.applied = want.changes;
        report
            .applied
            .push(Change::command("nft -f (vpsguard table)"));
        Ok(report)
    }

    async fn rollback(&self, ctx: &Context) -> Result<()> {
        let backup = ctx.path(BACKUP);
        let body = match tokio::fs::read_to_string(&backup).await {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(Error::Module {
                    module: "firewall".into(),
                    message: "no snapshot to roll back to".into(),
                });
            }
            Err(e) => return Err(Error::io(backup.display().to_string(), e)),
        };

        // `delete table` fails if absent; ignore that, then re-apply the snapshot.
        let staged = ctx.path(STAGED);
        write(&staged, &body).await?;
        let staged_str = staged.to_string_lossy().into_owned();
        ctx.runner().run("nft", &["-f", &staged_str]).await?;
        let _ = tokio::fs::remove_file(&backup).await;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Pure logic (no IO), unit-tested below.
// ---------------------------------------------------------------------------

/// A rendered ruleset plus its hash and the human-readable change list.
struct Ruleset {
    script: String,
    hash: String,
    changes: Vec<Change>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Proto {
    Tcp,
    Udp,
}

impl Proto {
    fn as_str(self) -> &'static str {
        match self {
            Proto::Tcp => "tcp",
            Proto::Udp => "udp",
        }
    }
}

/// A parsed allow rule, e.g. `80/tcp` or `22/tcp from 10.0.0.0/8`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct AllowRule {
    proto: Proto,
    port: u16,
    from: Option<String>,
}

impl AllowRule {
    fn to_nft(&self) -> String {
        let mut s = String::new();
        if let Some(cidr) = &self.from {
            let fam = if cidr.contains(':') { "ip6" } else { "ip" };
            s.push_str(&format!("{fam} saddr {cidr} "));
        }
        s.push_str(&format!(
            "{} dport {} accept",
            self.proto.as_str(),
            self.port
        ));
        s
    }

    fn describe(&self) -> String {
        match &self.from {
            Some(c) => format!("allow {}/{} from {c}", self.port, self.proto.as_str()),
            None => format!("allow {}/{}", self.port, self.proto.as_str()),
        }
    }
}

fn parse_rule(raw: &str) -> Result<AllowRule> {
    let (spec, from) = match raw.split_once(" from ") {
        Some((s, c)) => (s.trim(), Some(c.trim().to_string())),
        None => (raw.trim(), None),
    };
    let (port_s, proto_s) = spec
        .split_once('/')
        .ok_or_else(|| Error::Config(format!("firewall rule `{raw}` must be `port/proto`")))?;
    let port: u16 = port_s
        .trim()
        .parse()
        .map_err(|_| Error::Config(format!("firewall rule `{raw}`: bad port")))?;
    let proto = match proto_s.trim().to_ascii_lowercase().as_str() {
        "tcp" => Proto::Tcp,
        "udp" => Proto::Udp,
        other => {
            return Err(Error::Config(format!(
                "firewall rule `{raw}`: bad proto `{other}`"
            )))
        }
    };
    Ok(AllowRule { proto, port, from })
}

/// Build the nft script, hash, and change list from config + the SSH port.
fn plan_ruleset(ctx: &Context) -> Result<Ruleset> {
    let cfg: &FirewallConfig = &ctx.config.firewall;
    let ssh_port = ctx.config.ssh.port;

    let rules: Vec<AllowRule> = cfg
        .allow
        .iter()
        .map(|r| parse_rule(r))
        .collect::<Result<_>>()?;
    let policy = match cfg.default {
        Policy::Deny => "drop",
        Policy::Allow => "accept",
    };

    let hash = hash_config(policy, ssh_port, &rules);

    let mut script = String::new();
    script.push_str(&format!("add table inet {TABLE}\n"));
    script.push_str(&format!("flush table inet {TABLE}\n"));
    script.push_str(&format!(
        "add chain inet {TABLE} input {{ type filter hook input priority 0; policy {policy}; }}\n"
    ));
    script.push_str(&format!(
        "add rule inet {TABLE} input ct state invalid drop\n"
    ));
    script.push_str(&format!(
        "add rule inet {TABLE} input ct state established,related accept\n"
    ));
    script.push_str(&format!(
        "add rule inet {TABLE} input iif \"lo\" accept {}\n",
        marker(&hash)
    ));
    script.push_str(&format!(
        "add rule inet {TABLE} input tcp dport {ssh_port} accept\n"
    ));
    for r in &rules {
        script.push_str(&format!("add rule inet {TABLE} input {}\n", r.to_nft()));
    }

    let mut changes = vec![
        Change::command(format!("input policy {policy}")),
        Change::command(format!("allow {ssh_port}/tcp (ssh lockout guard)")),
    ];
    changes.extend(rules.iter().map(|r| Change::command(r.describe())));

    Ok(Ruleset {
        script,
        hash,
        changes,
    })
}

/// nft rule comment carrying the config hash, used by `check`.
fn marker(hash: &str) -> String {
    format!("comment \"vpsguard:{hash}\"")
}

/// FNV-1a over a canonical config string; stable across runs.
fn hash_config(policy: &str, ssh_port: u16, rules: &[AllowRule]) -> String {
    let mut canon = format!("{policy}|{ssh_port}|");
    let mut parts: Vec<String> = rules.iter().map(AllowRule::to_nft).collect();
    parts.sort();
    canon.push_str(&parts.join(","));

    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in canon.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{h:016x}")
}

// ---------------------------------------------------------------------------
// IO
// ---------------------------------------------------------------------------

/// `nft list table inet vpsguard`, or `None` when the table does not exist.
async fn nft_list_table(ctx: &Context) -> Result<Option<String>> {
    let out = ctx
        .runner()
        .run("nft", &["list", "table", "inet", TABLE])
        .await?;
    if out.success() {
        Ok(Some(out.stdout))
    } else {
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(s: &str) -> AllowRule {
        parse_rule(s).unwrap()
    }

    #[test]
    fn parses_simple_rule() {
        assert_eq!(
            rule("80/tcp"),
            AllowRule {
                proto: Proto::Tcp,
                port: 80,
                from: None
            }
        );
    }

    #[test]
    fn parses_rule_with_source() {
        let r = rule("22/tcp from 10.0.0.0/8");
        assert_eq!(r.port, 22);
        assert_eq!(r.from.as_deref(), Some("10.0.0.0/8"));
        assert_eq!(r.to_nft(), "ip saddr 10.0.0.0/8 tcp dport 22 accept");
    }

    #[test]
    fn ipv6_source_uses_ip6() {
        let r = rule("443/tcp from 2001:db8::/32");
        assert!(r.to_nft().starts_with("ip6 saddr 2001:db8::/32"));
    }

    #[test]
    fn rejects_bad_proto_and_port() {
        assert!(parse_rule("80/sctp").is_err());
        assert!(parse_rule("notaport/tcp").is_err());
        assert!(parse_rule("80").is_err());
    }

    #[test]
    fn hash_is_order_independent() {
        let a = vec![rule("80/tcp"), rule("443/tcp")];
        let b = vec![rule("443/tcp"), rule("80/tcp")];
        assert_eq!(hash_config("drop", 22, &a), hash_config("drop", 22, &b));
    }

    #[test]
    fn hash_changes_with_policy_and_port() {
        let r = vec![rule("80/tcp")];
        assert_ne!(hash_config("drop", 22, &r), hash_config("accept", 22, &r));
        assert_ne!(hash_config("drop", 22, &r), hash_config("drop", 2222, &r));
    }
}
