//! Concrete configuration modules implementing `vpsguard_core::Module`.
//!
//! SSH is the reference module; the remaining Phase 1 modules (users, firewall,
//! updates, fail2ban) follow the same shape.

#![forbid(unsafe_code)]

mod ssh;

pub use ssh::SshModule;

pub fn all() -> Vec<Box<dyn vpsguard_core::Module>> {
    vec![Box::new(SshModule)]
}
