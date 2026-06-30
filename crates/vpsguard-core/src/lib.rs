//! Core trait and value types for the vpsguard configuration engine.
//!
//! Owns the [`Module`] trait, the [`Context`] passed to every call, and the
//! [`Status`]/[`Change`]/[`Report`] vocabulary used to describe work.

#![forbid(unsafe_code)]

mod catalog;
mod config;
mod context;
mod error;
mod module;
mod runner;
mod types;

pub use catalog::ModuleCatalog;
pub use config::{Config, Profile, SshConfig};
pub use context::Context;
pub use error::{Error, Result};
pub use module::{Category, Module};
pub use runner::{CommandRunner, Output, SystemRunner};
pub use types::{Change, ChangeKind, Report, State, Status};
