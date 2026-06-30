//! vpsguard CLI entry point.

use std::path::PathBuf;

use clap::{Parser, Subcommand};
use color_eyre::eyre::{Context as _, Result};
use vpsguard_core::{Config, Context, State};

mod render;

#[derive(Parser)]
#[command(name = "vpsguard", version, about = "Configure and harden a Linux VPS")]
struct Cli {
    /// Path to the configuration file.
    #[arg(short, long, default_value = "vpsguard.toml", global = true)]
    config: PathBuf,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Write a starter vpsguard.toml.
    Init,
    /// Show the changes that apply would make.
    Plan,
    /// Converge the system to the desired state.
    Apply {
        /// Print the plan without changing anything.
        #[arg(long)]
        dry_run: bool,
        /// Skip the confirmation prompt.
        #[arg(long)]
        yes: bool,
    },
    /// Report compliance of each module.
    Audit,
    /// Restore the state captured before the last apply.
    Rollback,
}

#[tokio::main]
async fn main() -> Result<()> {
    color_eyre::install()?;
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .with_target(false)
        .init();

    let cli = Cli::parse();
    match cli.command {
        Command::Plan => plan(&cli.config).await,
        Command::Init | Command::Apply { .. } | Command::Audit | Command::Rollback => {
            println!("not yet implemented; this MVP branch ships `plan`.");
            Ok(())
        }
    }
}

fn load_config(path: &PathBuf) -> Result<Config> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("reading config {}", path.display()))?;
    Config::from_toml(&raw).with_context(|| format!("parsing {}", path.display()))
}

async fn plan(config_path: &PathBuf) -> Result<()> {
    let config = load_config(config_path)?;
    let ctx = Context::system(config);

    let catalog = vpsguard_modules::catalog();
    let mut total = 0usize;
    for module in catalog.iter() {
        let status = module.check(&ctx).await?;
        let changes = if status.state == State::Drift {
            module.plan(&ctx).await?
        } else {
            Vec::new()
        };
        total += changes.len();
        render::module_plan(module, &status, &changes);
    }

    render::summary(total);
    Ok(())
}
