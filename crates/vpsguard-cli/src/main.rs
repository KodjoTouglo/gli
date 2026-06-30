//! vpsguard CLI entry point.

use std::path::PathBuf;

use clap::{Parser, Subcommand};
use color_eyre::eyre::{bail, Context as _, Result};
use vpsguard_core::{Config, Context, ModuleCatalog, State};

mod render;

const STARTER_CONFIG: &str = include_str!("../../../examples/configs/vpsguard.toml");

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
    Init {
        /// Overwrite an existing file.
        #[arg(long)]
        force: bool,
    },
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
    Rollback {
        /// Limit rollback to one module by name.
        module: Option<String>,
    },
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
        Command::Init { force } => cmd_init(&cli.config, force),
        Command::Plan => cmd_plan(&cli.config).await,
        Command::Apply { dry_run, yes } => cmd_apply(&cli.config, dry_run, yes).await,
        Command::Audit => cmd_audit(&cli.config).await,
        Command::Rollback { module } => cmd_rollback(&cli.config, module.as_deref()).await,
    }
}

fn load(config_path: &PathBuf) -> Result<(Context, ModuleCatalog)> {
    let raw = std::fs::read_to_string(config_path)
        .with_context(|| format!("reading config {}", config_path.display()))?;
    let config =
        Config::from_toml(&raw).with_context(|| format!("parsing {}", config_path.display()))?;
    Ok((Context::system(config), vpsguard_modules::catalog()))
}

fn cmd_init(config_path: &PathBuf, force: bool) -> Result<()> {
    if config_path.exists() && !force {
        bail!(
            "{} already exists; pass --force to overwrite",
            config_path.display()
        );
    }
    std::fs::write(config_path, STARTER_CONFIG)
        .with_context(|| format!("writing {}", config_path.display()))?;
    println!("wrote {}", config_path.display());
    Ok(())
}

async fn cmd_plan(config_path: &PathBuf) -> Result<()> {
    let (ctx, catalog) = load(config_path)?;
    let mut total = 0;
    for module in catalog.iter() {
        let status = module.check(&ctx).await?;
        let changes = pending(module, &ctx, &status).await?;
        total += changes.len();
        render::module_plan(module, &status, &changes);
    }
    render::summary(total);
    Ok(())
}

async fn cmd_apply(config_path: &PathBuf, dry_run: bool, yes: bool) -> Result<()> {
    let (ctx, catalog) = load(config_path)?;

    let mut total = 0;
    for module in catalog.iter() {
        let status = module.check(&ctx).await?;
        let changes = pending(module, &ctx, &status).await?;
        total += changes.len();
        render::module_plan(module, &status, &changes);
    }
    render::summary(total);

    if total == 0 {
        return Ok(());
    }
    if dry_run {
        println!("\ndry-run: no changes made.");
        return Ok(());
    }
    if !yes && !confirm()? {
        println!("aborted.");
        return Ok(());
    }

    println!();
    for module in catalog.iter() {
        match module.apply(&ctx, false).await {
            Ok(report) => render::apply_report(&report),
            Err(e) => {
                eprintln!("[x] {}: {e}", module.name());
                eprintln!("    attempting rollback of {}...", module.name());
                match module.rollback(&ctx).await {
                    Ok(()) => eprintln!("    rolled back {}.", module.name()),
                    Err(re) => eprintln!("    rollback failed: {re}"),
                }
                bail!("apply aborted on module `{}`", module.name());
            }
        }
    }
    Ok(())
}

async fn cmd_audit(config_path: &PathBuf) -> Result<()> {
    let (ctx, catalog) = load(config_path)?;
    let mut compliant = 0;
    let mut applicable = 0;
    for module in catalog.iter() {
        let status = module.check(&ctx).await?;
        if status.state != State::NotApplicable {
            applicable += 1;
        }
        if status.is_compliant() {
            compliant += 1;
        }
        render::audit_line(module, &status);
    }
    render::score(compliant, applicable);
    Ok(())
}

async fn cmd_rollback(config_path: &PathBuf, only: Option<&str>) -> Result<()> {
    let (ctx, catalog) = load(config_path)?;
    if let Some(name) = only {
        let module = catalog
            .get(name)
            .ok_or_else(|| color_eyre::eyre::eyre!("unknown module `{name}`"))?;
        module.rollback(&ctx).await?;
        println!("rolled back {name}.");
        return Ok(());
    }
    for module in catalog.iter() {
        match module.rollback(&ctx).await {
            Ok(()) => println!("rolled back {}.", module.name()),
            Err(e) => eprintln!("[-] {}: {e}", module.name()),
        }
    }
    Ok(())
}

/// Changes a module would apply, or empty when compliant / not applicable.
async fn pending(
    module: &dyn vpsguard_core::Module,
    ctx: &Context,
    status: &vpsguard_core::Status,
) -> Result<Vec<vpsguard_core::Change>> {
    if status.state == State::Drift {
        Ok(module.plan(ctx).await?)
    } else {
        Ok(Vec::new())
    }
}

fn confirm() -> Result<bool> {
    Ok(inquire::Confirm::new("Apply these changes?")
        .with_default(false)
        .prompt()?)
}
