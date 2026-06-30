//! Concrete configuration modules implementing `vpsguard_core::Module`.
//!
//! SSH is the reference module; the other modules (users, firewall, updates,
//! fail2ban) follow the same shape.

#![forbid(unsafe_code)]

mod common;
mod firewall;
mod ssh;

pub use firewall::FirewallModule;
pub use ssh::SshModule;

use vpsguard_core::ModuleCatalog;

/// Build the catalog of modules enabled for the current MVP.
pub fn catalog() -> ModuleCatalog {
    ModuleCatalog::new(vec![Box::new(SshModule), Box::new(FirewallModule)])
}
