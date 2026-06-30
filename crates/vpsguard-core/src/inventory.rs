//! Inventory of remote servers, parsed from a TOML file.
//!
//! Servers are keyed by a short name and carry connection details plus tags.
//! A selector picks one server by name, or a group of servers by tag.

use std::collections::BTreeMap;

use serde::Deserialize;

/// A set of named servers (`[servers.<name>]`).
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
pub struct Inventory {
    pub servers: BTreeMap<String, Server>,
}

/// One remote server.
#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Server {
    /// Hostname or IP to connect to.
    pub host: String,
    /// SSH port.
    pub port: u16,
    /// SSH user.
    pub user: String,
    /// Tags used to select groups of servers.
    pub tags: Vec<String>,
}

impl Default for Server {
    fn default() -> Self {
        Self {
            host: String::new(),
            port: 22,
            user: "root".to_string(),
            tags: Vec::new(),
        }
    }
}

impl Inventory {
    /// Parse an inventory TOML document.
    pub fn from_toml(input: &str) -> crate::Result<Self> {
        toml::from_str(input).map_err(|e| crate::Error::Config(e.to_string()))
    }

    /// Select servers by exact name, else by tag. Empty when nothing matches.
    pub fn select(&self, selector: &str) -> Vec<(&str, &Server)> {
        if let Some((name, server)) = self.servers.get_key_value(selector) {
            return vec![(name.as_str(), server)];
        }
        self.servers
            .iter()
            .filter(|(_, s)| s.tags.iter().any(|t| t == selector))
            .map(|(n, s)| (n.as_str(), s))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
[servers.web1]
host = "10.0.0.1"
user = "deploy"
tags = ["web", "prod"]

[servers.web2]
host = "10.0.0.2"
port = 2222
tags = ["web"]

[servers.db1]
host = "10.0.0.3"
tags = ["db", "prod"]
"#;

    #[test]
    fn parses_with_defaults() {
        let inv = Inventory::from_toml(SAMPLE).unwrap();
        assert_eq!(inv.servers.len(), 3);
        assert_eq!(inv.servers["web1"].user, "deploy");
        assert_eq!(inv.servers["web2"].port, 2222);
        // Defaults: port 22, user root.
        assert_eq!(inv.servers["db1"].port, 22);
        assert_eq!(inv.servers["db1"].user, "root");
    }

    #[test]
    fn selects_by_name() {
        let inv = Inventory::from_toml(SAMPLE).unwrap();
        let sel = inv.select("web1");
        assert_eq!(sel.len(), 1);
        assert_eq!(sel[0].0, "web1");
    }

    #[test]
    fn selects_by_tag() {
        let inv = Inventory::from_toml(SAMPLE).unwrap();
        let web: Vec<&str> = inv.select("web").into_iter().map(|(n, _)| n).collect();
        assert_eq!(web, vec!["web1", "web2"]);
        let prod: Vec<&str> = inv.select("prod").into_iter().map(|(n, _)| n).collect();
        assert_eq!(prod, vec!["db1", "web1"]);
    }

    #[test]
    fn unknown_selector_is_empty() {
        let inv = Inventory::from_toml(SAMPLE).unwrap();
        assert!(inv.select("nope").is_empty());
    }
}
