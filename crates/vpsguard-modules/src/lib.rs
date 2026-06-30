//! Concrete configuration modules implementing `vpsguard_core::Module`.
//!
//! SSH is the reference module; the other modules (users, firewall, updates,
//! fail2ban, docker, caddy) follow the same shape.

#![forbid(unsafe_code)]

mod caddy;
mod common;
mod docker;
mod fail2ban;
mod firewall;
mod ssh;
mod tailscale;
mod updates;
mod users;

pub use caddy::CaddyModule;
pub use docker::DockerModule;
pub use fail2ban::Fail2banModule;
pub use firewall::FirewallModule;
pub use ssh::SshModule;
pub use tailscale::TailscaleModule;
pub use updates::UpdatesModule;
pub use users::UsersModule;

use vpsguard_core::ModuleCatalog;

/// Build the catalog of modules enabled for the current MVP.
pub fn catalog() -> ModuleCatalog {
    ModuleCatalog::new(vec![
        Box::new(SshModule),
        Box::new(FirewallModule),
        Box::new(UsersModule),
        Box::new(UpdatesModule),
        Box::new(Fail2banModule),
        Box::new(DockerModule),
        Box::new(CaddyModule),
        Box::new(TailscaleModule),
    ])
}
