# vpsguard

Configure and secure a fresh Linux VPS in one command.

vpsguard is a single static Rust binary that hardens and provisions a server
from a declarative `vpsguard.toml`. Every change is idempotent, previewed before
it runs, and reversible. It targets the gap left by fragile bash scripts (no
safety, no idempotence), Ansible (too heavy for one box), and PaaS tools like
Coolify (deploy apps but don't secure the OS).

> Status: MVP. Local execution against Debian/Ubuntu, Fedora, Rocky/RHEL,
> Arch, and openSUSE. Remote (agentless) execution and a TUI are on the roadmap.

## Why

When you rent a new VPS there is a pile of setup to do before it is safe or
useful: lock down SSH, put up a firewall, create a deploy user, turn on
automatic updates, add fail2ban, install Docker. vpsguard does all of it from
one config file, safely and repeatably.

## Principles

- **Idempotent** — every action can be re-run any number of times with no side
  effects once converged.
- **Preview first** — `plan` shows the diff; `apply` asks for confirmation
  before touching anything.
- **Reversible** — each module snapshots state before changing it and supports
  `rollback`.
- **Lockout-safe** — risky changes are validated before they take effect (SSH
  config with `sshd -t`, firewall ruleset with `nft -c`), and the firewall
  always keeps the SSH port open.
- **Single binary** — no Python or Ruby on the target.
- **Cross-distro** — detects the host and adapts service names and package
  managers.

## Install

Prebuilt binary (Linux x86_64/arm64, macOS Intel/Apple Silicon):

```sh
curl -fsSL https://raw.githubusercontent.com/KodjoTouglo/hardn/develop/install.sh | sh
```

Windows: download the `.zip` from the [Releases](https://github.com/KodjoTouglo/hardn/releases) page.

With Cargo:

```sh
cargo install --git https://github.com/KodjoTouglo/hardn vpsguard-cli
```

From source (Rust 1.85+):

```sh
git clone https://github.com/KodjoTouglo/hardn.git
cd hardn
cargo build --release --bin vpsguard
# binary at target/release/vpsguard
```

Tagged releases (`v*`) publish prebuilt binaries and checksums via GitHub
Actions. Homebrew, winget, npm, and PyPI packages that wrap these artifacts are
planned.

## Quickstart

```sh
# 1. Write a starter config
vpsguard init

# 2. Preview the changes (read-only)
vpsguard plan

# 3. Apply them (asks for confirmation; use --yes to skip)
sudo vpsguard apply

# Or preview an apply without changing anything
sudo vpsguard apply --dry-run
```

Example `plan` output:

```
[~] ssh (Harden sshd: custom port, no root/password login, modern crypto)
    ~ Port: (unset) -> 2222
    ~ PermitRootLogin: (unset) -> no
    ~ PasswordAuthentication: (unset) -> no
[~] firewall (nftables: default-deny input, allow listed ports, SSH protected)
    + input policy drop
    + allow 2222/tcp (ssh lockout guard)
    + allow 80/tcp
    + allow 443/tcp
[~] users (Create users, grant sudo via sudoers.d, install SSH keys)
    + create user deploy
    + grant sudo to deploy

9 changes pending. Run `vpsguard apply` to converge.
```

## Commands

| Command | Description |
|---|---|
| `init` | Write a starter `vpsguard.toml` (`--force` to overwrite) |
| `plan` | Show the changes `apply` would make (read-only) |
| `apply` | Converge the system (`--dry-run`, `--yes`) |
| `audit` | Report per-module compliance and a security score |
| `rollback [module]` | Restore state from before the last apply |
| `recipes` | List the builtin recipes |

## Modules

| Module | Category | What it does |
|---|---|---|
| `ssh` | Security | Custom port, disable root/password login, modern ciphers; validated with `sshd -t` |
| `firewall` | Security | nftables default-deny input, allow-list, SSH always permitted |
| `users` | Security | Create users, grant sudo via `sudoers.d`, install SSH keys |
| `updates` | System | Automatic security updates (unattended-upgrades / dnf-automatic) |
| `fail2ban` | Security | Install fail2ban, enable jails (sshd tracks the SSH port) |
| `docker` | Runtime | Install Docker, enable the service, add users to the docker group (opt-in) |

Security modules form an always-on baseline; provisioning modules like `docker`
are opt-in.

## Configuration

`vpsguard.toml`:

```toml
profile = "balanced"   # homelab | balanced | strict | paranoid

[ssh]
port = 2222
permit_root_login = false
password_auth = false
modern_ciphers = true

[firewall]
enabled = true
backend = "nftables"
default = "deny"
allow = ["80/tcp", "443/tcp", "22/tcp from 10.0.0.0/8"]

[users.deploy]
sudo = true
ssh_keys = ["ssh-ed25519 AAAA... deploy@host"]

[updates]
enabled = true
auto_reboot = "02:00"

[fail2ban]
enabled = true
jails = ["sshd"]
bantime = "10m"
maxretry = 5

# Opt-in provisioning
[docker]
enabled = false
users = ["deploy"]
```

### Recipes

A recipe is a named preset. Set `recipe = "<name>"` and your own keys are
layered on top (your values win, unspecified keys inherit the preset):

```toml
recipe = "web-server"

[ssh]
port = 2222          # override the preset's default
```

```sh
vpsguard recipes      # list builtin recipes
```

Builtin: `baseline` (SSH hardening, default-deny firewall, fail2ban,
auto-updates) and `web-server` (baseline plus inbound 80/443).

## Cross-distro support

vpsguard reads `/etc/os-release` and adapts:

| Family | SSH service | Package manager |
|---|---|---|
| Debian/Ubuntu | `ssh` | `apt-get` |
| Fedora/Rocky/RHEL | `sshd` | `dnf` |
| Arch | `sshd` | `pacman` |
| openSUSE | `sshd` | `zypper` |

The firewall uses nftables, which is present across all of them.

## Architecture

Cargo workspace:

```
crates/
  vpsguard-core/      Module trait, Context, Status/Change/Report, Platform, recipes
  vpsguard-modules/   ssh, firewall, users, updates, fail2ban, docker
  vpsguard-cli/       clap-based CLI (the vpsguard binary)
  vpsguard-tui/       ratatui front-end (planned)
  vpsguard-agent/     remote execution over SSH via russh (planned)
```

Every module implements one trait:

```rust
#[async_trait]
pub trait Module: Send + Sync {
    fn name(&self) -> &str;
    fn summary(&self) -> &str;
    fn category(&self) -> Category;
    async fn check(&self, ctx: &Context) -> Result<Status>;
    async fn plan(&self, ctx: &Context) -> Result<Vec<Change>>;
    async fn apply(&self, ctx: &Context, dry_run: bool) -> Result<Report>;
    async fn rollback(&self, ctx: &Context) -> Result<()>;
}
```

The `Context` injects the filesystem root and a command runner, so modules are
unit-tested against a tempdir with a mock runner; no root or real services
needed.

## Roadmap

- Remote agentless execution (`--target`, `--group`) over russh
- Interactive TUI (dashboard, security score, plan diff)
- More provisioning: k3s, TLS/ACME, Tailscale
- Local state history (SQLite) and timed auto-rollback for SSH/firewall
- Compliance reports (CIS, ANSSI)

## Development

```sh
cargo fmt --all
cargo clippy --workspace --all-targets --all-features
cargo test --workspace
```

## License

Apache-2.0. See [LICENSE](LICENSE).
