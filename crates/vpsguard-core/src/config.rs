//! Deserializable configuration mirroring `vpsguard.toml`.
//!
//! Only the fields needed by the MVP modules are modelled. Unknown tables are
//! ignored so the format can grow without breaking older binaries.

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
    pub profile: Profile,
    pub ssh: SshConfig,
    pub firewall: FirewallConfig,
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
}
