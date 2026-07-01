//! Lockout protection.
//!
//! After applying a risky change (ssh, firewall), a guard process is spawned
//! detached via `setsid` so it survives an SSH hangup. It rolls the change back
//! unless the operator confirms from the original session within a timeout. If
//! the operator locks themselves out, their session dies, no confirmation
//! arrives, and the guard restores the previous state.

use std::path::{Path, PathBuf};
use std::time::Duration;

use color_eyre::eyre::Result;
use gli_core::{Context, ModuleCatalog};
use tokio::io::{AsyncBufReadExt, BufReader};

const TIMEOUT_SECS: u64 = 60;

/// Spawn the guard for `modules`, then prompt the operator to confirm.
pub async fn confirm_or_rollback(config: &Path, modules: &[String]) -> Result<()> {
    let commit = commit_path();
    let _ = std::fs::remove_file(&commit);

    if !spawn_guard(config, modules, &commit) {
        eprintln!(
            "warning: lockout guard unavailable (setsid missing); applied risky \
             changes without an auto-rollback safety net."
        );
        return Ok(());
    }

    println!(
        "\nRisky changes applied ({}). They roll back automatically in {TIMEOUT_SECS}s \
         unless you confirm from this session.",
        modules.join(", ")
    );
    print!("Press Enter to keep them: ");
    use std::io::Write;
    let _ = std::io::stdout().flush();

    if read_confirmation(TIMEOUT_SECS - 5).await {
        std::fs::write(&commit, b"ok").ok();
        println!("changes kept.");
    } else {
        println!("no confirmation; the guard is rolling back.");
    }
    Ok(())
}

/// The detached guard: wait for the commit file up to `timeout`, else roll back.
pub async fn run_guard(
    ctx: &Context,
    catalog: &ModuleCatalog,
    modules: &[String],
    commit: &Path,
    timeout: u64,
) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(timeout);
    loop {
        if commit.exists() {
            return; // operator confirmed; keep the changes
        }
        if tokio::time::Instant::now() >= deadline {
            break;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    for name in modules {
        if let Some(m) = catalog.get(name) {
            match m.rollback(ctx).await {
                Ok(()) => eprintln!("lockout: rolled back {name}"),
                Err(e) => eprintln!("lockout: rollback of {name} failed: {e}"),
            }
        }
    }
    let _ = std::fs::remove_file(commit);
}

fn commit_path() -> PathBuf {
    std::env::temp_dir().join(format!("gli-commit-{}", std::process::id()))
}

fn spawn_guard(config: &Path, modules: &[String], commit: &Path) -> bool {
    let Ok(exe) = std::env::current_exe() else {
        return false;
    };
    std::process::Command::new("setsid")
        .arg(exe)
        .arg("-c")
        .arg(config)
        .arg("guard")
        .arg("--modules")
        .arg(modules.join(","))
        .arg("--timeout")
        .arg(TIMEOUT_SECS.to_string())
        .arg("--commit")
        .arg(commit)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .is_ok()
}

async fn read_confirmation(secs: u64) -> bool {
    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    match tokio::time::timeout(Duration::from_secs(secs), lines.next_line()).await {
        Ok(Ok(Some(l))) => {
            let l = l.trim().to_lowercase();
            l.is_empty() || l == "y" || l == "yes"
        }
        _ => false,
    }
}
