//! Command execution abstraction.
//!
//! Modules never call [`std::process::Command`] directly; they go through a
//! [`CommandRunner`]. This makes `apply`/`rollback` paths unit-testable with a
//! mock and keeps a single choke point for future remote (russh) execution.

use async_trait::async_trait;

use crate::{Error, Result};

/// Captured result of a command invocation.
#[derive(Debug, Clone)]
pub struct Output {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}

impl Output {
    pub fn success(&self) -> bool {
        self.code == 0
    }
}

/// Runs external commands. Implemented by the real system and by test mocks.
#[async_trait]
pub trait CommandRunner: Send + Sync {
    async fn run(&self, command: &str, args: &[&str]) -> Result<Output>;

    /// Run and fail with [`Error::Command`] on a non-zero exit.
    async fn run_checked(&self, command: &str, args: &[&str]) -> Result<Output> {
        let out = self.run(command, args).await?;
        if out.success() {
            Ok(out)
        } else {
            Err(Error::Command {
                command: format!("{command} {}", args.join(" ")),
                code: out.code,
                stderr: out.stderr,
            })
        }
    }
}

/// Executes commands on the local host via Tokio.
#[derive(Debug, Default)]
pub struct SystemRunner;

#[async_trait]
impl CommandRunner for SystemRunner {
    async fn run(&self, command: &str, args: &[&str]) -> Result<Output> {
        let out = tokio::process::Command::new(command)
            .args(args)
            .output()
            .await
            .map_err(|e| Error::Command {
                command: format!("{command} {}", args.join(" ")),
                code: -1,
                stderr: e.to_string(),
            })?;

        Ok(Output {
            code: out.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        })
    }
}
