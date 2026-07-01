//! Builtin recipes: named config presets a user can start from.
//!
//! A recipe is a partial vpsguard.toml. Selecting `recipe = "name"` uses it as
//! the base, with the user's own tables layered on top (user keys win). This is
//! pure composition over the existing modules, not a new execution path.

/// A builtin recipe: stable name, one-line description, preset TOML.
pub struct Recipe {
    pub name: &'static str,
    pub description: &'static str,
    pub preset: &'static str,
}

const BASELINE: &str = r#"
profile = "balanced"

[firewall]
allow = []
"#;

const WEB_SERVER: &str = r#"
profile = "balanced"

[firewall]
allow = ["80/tcp", "443/tcp"]

[fail2ban]
jails = ["sshd"]
"#;

const DOCKER_HOST: &str = r#"
profile = "balanced"

[firewall]
allow = []

[docker]
enabled = true
"#;

const WORDPRESS: &str = r#"
profile = "balanced"

[firewall]
allow = ["80/tcp", "443/tcp"]

[docker]
enabled = true

[app]
enabled = true
framework = "wordpress"
"#;

const BUILTINS: &[Recipe] = &[
    Recipe {
        name: "baseline",
        description: "SSH hardening, default-deny firewall, fail2ban, auto-updates",
        preset: BASELINE,
    },
    Recipe {
        name: "web-server",
        description: "Baseline plus inbound 80/443 for a public web server",
        preset: WEB_SERVER,
    },
    Recipe {
        name: "docker-host",
        description: "Baseline plus Docker installed and enabled",
        preset: DOCKER_HOST,
    },
    Recipe {
        name: "wordpress",
        description: "Baseline plus a Docker WordPress + MariaDB stack (set app.domain for HTTPS)",
        preset: WORDPRESS,
    },
];

/// All builtin recipes.
pub fn all() -> &'static [Recipe] {
    BUILTINS
}

/// Preset TOML for `name`, if it exists.
pub fn preset(name: &str) -> Option<&'static str> {
    BUILTINS.iter().find(|r| r.name == name).map(|r| r.preset)
}

/// Recipe names, for error messages and listings.
pub fn names() -> Vec<&'static str> {
    BUILTINS.iter().map(|r| r.name).collect()
}
