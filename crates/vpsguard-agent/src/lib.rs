//! Agentless remote execution over SSH (russh).
//!
//! Connects to a remote host, then exposes a [`CommandRunner`] and a
//! [`FileSystem`] backed by that SSH session. The same modules that run locally
//! run unchanged against the remote: commands go over an exec channel and file
//! IO uses small shell commands (cat / base64 / mv / rm / test), so no agent or
//! SFTP subsystem is required on the target. All interpolated values are
//! single-quote escaped by `quote`.

#![forbid(unsafe_code)]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use base64::Engine;
use russh::{client, ChannelMsg};
use tokio::sync::Mutex;
use vpsguard_core::{
    CommandRunner, Config, Context, Error as CoreError, FileSystem, Output, Platform,
};

/// Errors raised while establishing a connection.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("ssh connect to {target} failed: {source}")]
    Connect {
        target: String,
        #[source]
        source: russh::Error,
    },

    #[error("authentication failed for {user}@{host}")]
    Auth { user: String, host: String },
}

/// How to authenticate to a host.
#[derive(Debug, Clone)]
pub enum Auth {
    Password(String),
}

/// An open SSH session.
pub struct SshConn {
    handle: Mutex<client::Handle<AcceptAllKeys>>,
}

impl SshConn {
    /// Connect and authenticate.
    pub async fn connect(
        host: &str,
        port: u16,
        user: &str,
        auth: &Auth,
    ) -> Result<Arc<Self>, Error> {
        let config = Arc::new(client::Config::default());
        let mut handle = client::connect(config, (host, port), AcceptAllKeys)
            .await
            .map_err(|e| Error::Connect {
                target: format!("{host}:{port}"),
                source: e,
            })?;

        let ok = match auth {
            Auth::Password(p) => {
                handle
                    .authenticate_password(user, p)
                    .await
                    .map_err(|e| Error::Connect {
                        target: format!("{host}:{port}"),
                        source: e,
                    })?
            }
        };
        if !ok {
            return Err(Error::Auth {
                user: user.to_string(),
                host: host.to_string(),
            });
        }
        Ok(Arc::new(Self {
            handle: Mutex::new(handle),
        }))
    }

    /// Run a shell command, capturing exit code, stdout, and stderr.
    async fn exec(&self, command: &str) -> Result<Output, CoreError> {
        let exec_err = |e: russh::Error| CoreError::Command {
            command: command.to_string(),
            code: -1,
            stderr: e.to_string(),
        };

        let handle = self.handle.lock().await;
        let mut channel = handle.channel_open_session().await.map_err(exec_err)?;
        channel.exec(true, command).await.map_err(exec_err)?;

        let mut code = -1;
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        while let Some(msg) = channel.wait().await {
            match msg {
                ChannelMsg::Data { ref data } => stdout.extend_from_slice(data),
                ChannelMsg::ExtendedData { ref data, ext } => {
                    if ext == 1 {
                        stderr.extend_from_slice(data);
                    }
                }
                ChannelMsg::ExitStatus { exit_status } => code = exit_status as i32,
                ChannelMsg::Eof | ChannelMsg::Close => {}
                _ => {}
            }
        }

        Ok(Output {
            code,
            stdout: String::from_utf8_lossy(&stdout).into_owned(),
            stderr: String::from_utf8_lossy(&stderr).into_owned(),
        })
    }

    /// Detect the remote distribution from /etc/os-release.
    async fn detect_platform(&self) -> Platform {
        let os = self
            .exec("cat /etc/os-release")
            .await
            .map(|o| o.stdout)
            .unwrap_or_default();
        Platform::from_os_release(&os)
    }
}

/// Accepts any host key (trust-on-first-use is not yet implemented).
pub struct AcceptAllKeys;

#[async_trait]
impl client::Handler for AcceptAllKeys {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        _server_public_key: &russh::keys::key::PublicKey,
    ) -> Result<bool, Self::Error> {
        Ok(true)
    }
}

/// Runs commands on the remote host.
pub struct RemoteRunner {
    conn: Arc<SshConn>,
}

#[async_trait]
impl CommandRunner for RemoteRunner {
    async fn run(&self, command: &str, args: &[&str]) -> Result<Output, CoreError> {
        let mut cmd = String::from(command);
        for a in args {
            cmd.push(' ');
            cmd.push_str(&quote(a));
        }
        self.conn.exec(&cmd).await
    }
}

/// Remote filesystem via shell commands over the SSH session.
pub struct RemoteFs {
    conn: Arc<SshConn>,
}

#[async_trait]
impl FileSystem for RemoteFs {
    async fn read(&self, path: &Path) -> Result<Option<String>, CoreError> {
        let out = self
            .conn
            .exec(&format!("cat -- {}", quote(&path.to_string_lossy())))
            .await?;
        Ok(out.success().then_some(out.stdout))
    }

    async fn write(&self, path: &Path, body: &str) -> Result<(), CoreError> {
        let p = path.to_string_lossy();
        let dir = path.parent().map(|d| d.to_string_lossy().into_owned());
        let b64 = base64::engine::general_purpose::STANDARD.encode(body);
        let mkdir = dir
            .map(|d| format!("mkdir -p {} && ", quote(&d)))
            .unwrap_or_default();
        let cmd = format!(
            "{mkdir}printf %s {} | base64 -d > {}",
            quote(&b64),
            quote(&p)
        );
        let out = self.conn.exec(&cmd).await?;
        check(out, &cmd)
    }

    async fn remove(&self, path: &Path) -> Result<(), CoreError> {
        let cmd = format!("rm -f -- {}", quote(&path.to_string_lossy()));
        let out = self.conn.exec(&cmd).await?;
        check(out, &cmd)
    }

    async fn exists(&self, path: &Path) -> Result<bool, CoreError> {
        let out = self
            .conn
            .exec(&format!("test -e {}", quote(&path.to_string_lossy())))
            .await?;
        Ok(out.success())
    }

    async fn rename(&self, from: &Path, to: &Path) -> Result<(), CoreError> {
        let cmd = format!(
            "mv -- {} {}",
            quote(&from.to_string_lossy()),
            quote(&to.to_string_lossy())
        );
        let out = self.conn.exec(&cmd).await?;
        check(out, &cmd)
    }
}

/// Build a [`Context`] that runs against a remote host.
pub async fn remote_context(
    config: Config,
    host: &str,
    port: u16,
    user: &str,
    auth: &Auth,
) -> Result<Context, Error> {
    let conn = SshConn::connect(host, port, user, auth).await?;
    let platform = conn.detect_platform().await;
    let runner = Arc::new(RemoteRunner { conn: conn.clone() });
    let fs = Arc::new(RemoteFs { conn });
    Ok(Context::with_parts(config, PathBuf::from("/"), runner)
        .with_fs(fs)
        .with_platform(platform))
}

fn check(out: Output, cmd: &str) -> Result<(), CoreError> {
    if out.success() {
        Ok(())
    } else {
        Err(CoreError::Command {
            command: cmd.to_string(),
            code: out.code,
            stderr: out.stderr,
        })
    }
}

/// POSIX single-quote a string for safe shell interpolation.
fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

#[cfg(test)]
mod tests {
    use super::quote;

    #[test]
    fn quoting_wraps_and_escapes() {
        assert_eq!(quote("/etc/ssh"), "'/etc/ssh'");
        assert_eq!(quote("a'b"), "'a'\\''b'");
    }
}
