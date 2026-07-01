//! Crate-wide error type and `Result` alias.

use thiserror::Error;

/// Errors raised by core logic and modules.
#[derive(Debug, Error)]
pub enum Error {
    #[error("io error at {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },

    #[error("config error: {0}")]
    Config(String),

    #[error("command `{command}` failed (exit {code}): {stderr}")]
    Command {
        command: String,
        code: i32,
        stderr: String,
    },

    /// A precondition that protects against lockout was not satisfied.
    #[error("safety check failed: {0}")]
    Safety(String),

    #[error("module `{module}`: {message}")]
    Module { module: String, message: String },
}

impl Error {
    /// Helper to build an [`Error::Io`] carrying the offending path.
    pub fn io(path: impl Into<String>, source: std::io::Error) -> Self {
        Error::Io {
            path: path.into(),
            source,
        }
    }
}

/// Convenience result alias used throughout the workspace.
pub type Result<T> = std::result::Result<T, Error>;
