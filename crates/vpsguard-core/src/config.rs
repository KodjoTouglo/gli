//! Deserializable configuration mirroring `vpsguard.toml`.
//!
//! Only the fields needed by the MVP modules are modelled. Unknown tables are
//! ignored so the format can grow without breaking older binaries.

use std::collections::BTreeMap;

use serde::Deserialize;

/// Hardening profile. Selects the default strictness of modules.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Profile {
    Homelab,
    #[default]
    Balanced,
    Strict,
    Paranoid,
}

/// Root configuration document.
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// Optional builtin recipe to use as a base preset.
    pub recipe: Option<String>,
    pub profile: Profile,
    pub ssh: SshConfig,
    pub firewall: FirewallConfig,
    /// Managed users, keyed by username (`[users.deploy]`).
    pub users: BTreeMap<String, UserConfig>,
    pub updates: UpdatesConfig,
    pub fail2ban: Fail2banConfig,
    pub docker: DockerConfig,
    pub caddy: CaddyConfig,
    pub system: SystemConfig,
    pub tailscale: TailscaleConfig,
    pub postgres: PostgresConfig,
    pub redis: RedisConfig,
}

/// Base system settings (`[system]`): hostname, timezone, swap.
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
pub struct SystemConfig {
    /// Static hostname to set.
    pub hostname: Option<String>,
    /// Timezone, e.g. "Europe/Paris".
    pub timezone: Option<String>,
    /// Swap file size in MiB; None or 0 leaves swap unmanaged.
    pub swap_mb: Option<u32>,
}

/// Tailscale settings (`[tailscale]`).
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
pub struct TailscaleConfig {
    /// Install Tailscale and bring the node up.
    pub enabled: bool,
    /// Pre-auth key for non-interactive `tailscale up`.
    pub auth_key: Option<String>,
    /// Enable Tailscale SSH on this node.
    pub ssh: bool,
}

/// PostgreSQL settings (`[postgres]`).
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
pub struct PostgresConfig {
    /// Install PostgreSQL and enable its service.
    pub enabled: bool,
    /// Databases to create if missing.
    pub databases: Vec<String>,
}

/// Redis settings (`[redis]`).
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
pub struct RedisConfig {
    /// Install Redis and enable its service.
    pub enabled: bool,
}

/// Docker runtime settings (`[docker]`).
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
pub struct DockerConfig {
    /// Install Docker and enable its service.
    pub enabled: bool,
    /// Users to add to the docker group.
    pub users: Vec<String>,
}

/// Caddy reverse-proxy settings (`[caddy]`).
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
pub struct CaddyConfig {
    /// Install Caddy and manage its Caddyfile.
    pub enabled: bool,
    /// Sites to serve (`[[caddy.sites]]`).
    pub sites: Vec<CaddySite>,
}

/// One Caddy site block. Automatic HTTPS applies to public domains.
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
pub struct CaddySite {
    /// Domain to serve, e.g. "example.com".
    pub domain: String,
    /// Upstream to reverse-proxy to, e.g. "localhost:8080".
    pub reverse_proxy: Option<String>,
    /// Directory to serve statically (used when reverse_proxy is unset).
    pub root: Option<String>,
}

/// fail2ban settings (`[fail2ban]`).
#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Fail2banConfig {
    /// Install and enable fail2ban.
    pub enabled: bool,
    /// Jails to enable, e.g. ["sshd"].
    pub jails: Vec<String>,
    /// Ban duration (fail2ban syntax, e.g. "10m"); None uses fail2ban default.
    pub bantime: Option<String>,
    /// Failures before a ban; None uses fail2ban default.
    pub maxretry: Option<u32>,
}

impl Default for Fail2banConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            jails: vec!["sshd".to_string()],
            bantime: None,
            maxretry: None,
        }
    }
}

/// Automatic update settings (`[updates]`).
#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct UpdatesConfig {
    /// Enable unattended security updates.
    pub enabled: bool,
    /// Daily reboot time "HH:MM" when updates require it; None disables reboot.
    pub auto_reboot: Option<String>,
}

impl Default for UpdatesConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            auto_reboot: None,
        }
    }
}

/// A managed user account (`[users.<name>]`).
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
pub struct UserConfig {
    /// Grant passwordless-free sudo via /etc/sudoers.d.
    pub sudo: bool,
    /// Authorized SSH public keys to install.
    pub ssh_keys: Vec<String>,
}

/// SSH daemon hardening settings (`[ssh]`).
#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SshConfig {
    /// Port `sshd` listens on.
    pub port: u16,
    /// Whether root may log in over SSH.
    pub permit_root_login: bool,
    /// Whether password authentication is allowed.
    pub password_auth: bool,
    /// Apply a modern, restrictive cipher/kex/mac set.
    pub modern_ciphers: bool,
}

impl Default for SshConfig {
    fn default() -> Self {
        // Secure-by-default: custom port discouraged, so keep 22 unless set;
        // root and password login off.
        Self {
            port: 22,
            permit_root_login: false,
            password_auth: false,
            modern_ciphers: true,
        }
    }
}

/// Firewall backend. Only nftables is supported for now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum FirewallBackend {
    #[default]
    Nftables,
}

/// Default policy for the input chain.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Policy {
    #[default]
    Deny,
    Allow,
}

/// Firewall settings (`[firewall]`).
#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct FirewallConfig {
    /// Whether vpsguard manages the firewall at all.
    pub enabled: bool,
    /// Backend used to program rules.
    pub backend: FirewallBackend,
    /// Input policy when no allow rule matches.
    pub default: Policy,
    /// Allow rules, e.g. "80/tcp", "443/tcp", "22/tcp from 10.0.0.0/8".
    pub allow: Vec<String>,
}

impl Default for FirewallConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            backend: FirewallBackend::Nftables,
            default: Policy::Deny,
            allow: Vec::new(),
        }
    }
}

impl Config {
    /// Parse a TOML document into a [`Config`].
    pub fn from_toml(input: &str) -> crate::Result<Self> {
        toml::from_str(input).map_err(|e| crate::Error::Config(e.to_string()))
    }

    /// Parse, expanding a `recipe = "name"` preset under the user's own keys.
    pub fn resolve(input: &str) -> crate::Result<Self> {
        let user: toml::Value =
            toml::from_str(input).map_err(|e| crate::Error::Config(e.to_string()))?;

        let merged = match user.get("recipe").and_then(|v| v.as_str()) {
            Some(name) => {
                let preset = crate::recipes::preset(name).ok_or_else(|| {
                    crate::Error::Config(format!(
                        "unknown recipe `{name}`; available: {}",
                        crate::recipes::names().join(", ")
                    ))
                })?;
                let base: toml::Value =
                    toml::from_str(preset).map_err(|e| crate::Error::Config(e.to_string()))?;
                deep_merge(base, user)
            }
            None => user,
        };

        merged
            .try_into()
            .map_err(|e: toml::de::Error| crate::Error::Config(e.to_string()))
    }
}

/// Recursively merge `over` onto `base`; `over` wins on scalars and arrays.
fn deep_merge(base: toml::Value, over: toml::Value) -> toml::Value {
    match (base, over) {
        (toml::Value::Table(mut b), toml::Value::Table(o)) => {
            for (k, v) in o {
                let merged = match b.remove(&k) {
                    Some(existing) => deep_merge(existing, v),
                    None => v,
                };
                b.insert(k, merged);
            }
            toml::Value::Table(b)
        }
        (_, over) => over,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recipe_presets_apply() {
        let c = Config::resolve("recipe = \"web-server\"").unwrap();
        assert_eq!(c.firewall.allow, vec!["80/tcp", "443/tcp"]);
    }

    #[test]
    fn docker_host_recipe_enables_docker() {
        let c = Config::resolve("recipe = \"docker-host\"").unwrap();
        assert!(c.docker.enabled);
    }

    #[test]
    fn user_keys_override_recipe() {
        let raw = "recipe = \"web-server\"\n[firewall]\nallow = [\"8080/tcp\"]\n";
        let c = Config::resolve(raw).unwrap();
        // User's allow replaces the recipe's, but unspecified keys stay.
        assert_eq!(c.firewall.allow, vec!["8080/tcp"]);
        assert_eq!(c.firewall.default, Policy::Deny);
    }

    #[test]
    fn user_scalars_override_but_siblings_kept() {
        let raw = "recipe = \"web-server\"\n[ssh]\nport = 2222\n";
        let c = Config::resolve(raw).unwrap();
        assert_eq!(c.ssh.port, 2222);
        // Recipe's firewall preset survives a user [ssh] override.
        assert_eq!(c.firewall.allow, vec!["80/tcp", "443/tcp"]);
    }

    #[test]
    fn unknown_recipe_errors() {
        let err = Config::resolve("recipe = \"nope\"").unwrap_err();
        assert!(err.to_string().contains("unknown recipe"));
    }

    #[test]
    fn resolve_without_recipe_matches_from_toml() {
        let raw = "[ssh]\nport = 22\n";
        let a = Config::resolve(raw).unwrap();
        assert_eq!(a.ssh.port, 22);
        assert!(a.recipe.is_none());
    }
}
