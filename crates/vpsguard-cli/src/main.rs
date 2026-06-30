//! vpsguard CLI entry point.

use std::path::PathBuf;

use clap::{Parser, Subcommand};
use color_eyre::eyre::{bail, Context as _, Result};
use vpsguard_core::{Config, Context, ModuleCatalog, State};
use vpsguard_state::{History, DEFAULT_PATH};

mod lockout;
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
        /// Disable the post-apply lockout guard for risky modules.
        #[arg(long)]
        no_guard: bool,
    },
    /// Report compliance of each module.
    Audit,
    /// Restore the state captured before the last apply.
    Rollback {
        /// Limit rollback to one module by name.
        module: Option<String>,
    },
    /// List the builtin recipes.
    Recipes,
    /// Show recent apply/rollback events.
    History {
        /// Number of events to show.
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
    /// Internal: watch for confirmation and roll back risky modules on timeout.
    #[command(hide = true)]
    Guard {
        /// Comma-separated module names to roll back on timeout.
        #[arg(long)]
        modules: String,
        /// Seconds to wait for confirmation.
        #[arg(long)]
        timeout: u64,
        /// File whose creation signals the operator confirmed.
        #[arg(long)]
        commit: PathBuf,
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
        Command::Apply {
            dry_run,
            yes,
            no_guard,
        } => cmd_apply(&cli.config, dry_run, yes, no_guard).await,
        Command::Audit => cmd_audit(&cli.config).await,
        Command::Rollback { module } => cmd_rollback(&cli.config, module.as_deref()).await,
        Command::Recipes => {
            cmd_recipes();
            Ok(())
        }
        Command::History { limit } => {
            cmd_history(limit);
            Ok(())
        }
        Command::Guard {
            modules,
            timeout,
            commit,
        } => cmd_guard(&cli.config, &modules, timeout, &commit).await,
    }
}

/// Open the history store, best-effort: a warning instead of failing the run.
fn open_history() -> Option<History> {
    match History::open(DEFAULT_PATH) {
        Ok(h) => Some(h),
        Err(e) => {
            eprintln!("warning: history unavailable: {e}");
            None
        }
    }
}

fn cmd_history(limit: usize) {
    let Some(history) = open_history() else {
        return;
    };
    match history.recent(limit) {
        Ok(events) if events.is_empty() => println!("No history yet."),
        Ok(events) => {
            for e in events {
                let mark = if e.ok { "ok" } else { "x" };
                println!(
                    "{}  [{mark}] {:<9} {:<10} {}",
                    e.timestamp, e.action, e.module, e.summary
                );
            }
        }
        Err(e) => eprintln!("warning: could not read history: {e}"),
    }
}

async fn cmd_guard(
    config: &PathBuf,
    modules: &str,
    timeout: u64,
    commit: &std::path::Path,
) -> Result<()> {
    let (ctx, catalog) = load(config)?;
    let names: Vec<String> = modules.split(',').map(str::to_string).collect();
    lockout::run_guard(&ctx, &catalog, &names, commit, timeout).await;
    Ok(())
}

fn load(config_path: &PathBuf) -> Result<(Context, ModuleCatalog)> {
    let raw = std::fs::read_to_string(config_path)
        .with_context(|| format!("reading config {}", config_path.display()))?;
    let config =
        Config::resolve(&raw).with_context(|| format!("parsing {}", config_path.display()))?;
    Ok((Context::system(config), vpsguard_modules::catalog()))
}

fn cmd_recipes() {
    println!("Available recipes (set `recipe = \"<name>\"` in vpsguard.toml):\n");
    for r in vpsguard_core::recipes::all() {
        println!("  {:<12} {}", r.name, r.description);
    }
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

async fn cmd_apply(config_path: &PathBuf, dry_run: bool, yes: bool, no_guard: bool) -> Result<()> {
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
    let history = open_history();
    let mut risky = Vec::new();
    for module in catalog.iter() {
        match module.apply(&ctx, false).await {
            Ok(report) => {
                render::apply_report(&report);
                if let Some(h) = &history {
                    let summary = if report.is_noop() {
                        "no changes".to_string()
                    } else {
                        format!("{} change(s)", report.applied.len())
                    };
                    let _ = h.record("apply", module.name(), &summary, true);
                }
                if module.lockout_risk() && !report.is_noop() {
                    risky.push(module.name().to_string());
                }
            }
            Err(e) => {
                if let Some(h) = &history {
                    let _ = h.record("apply", module.name(), &e.to_string(), false);
                }
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

    // Interactive runs get a timed safety net; --yes (automation) and --no-guard opt out.
    if !risky.is_empty() && !yes && !no_guard {
        lockout::confirm_or_rollback(config_path, &risky).await?;
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
    let history = open_history();
    let record = |name: &str, ok: bool, detail: &str| {
        if let Some(h) = &history {
            let _ = h.record("rollback", name, detail, ok);
        }
    };

    if let Some(name) = only {
        let module = catalog
            .get(name)
            .ok_or_else(|| color_eyre::eyre::eyre!("unknown module `{name}`"))?;
        module.rollback(&ctx).await?;
        record(name, true, "rolled back");
        println!("rolled back {name}.");
        return Ok(());
    }
    for module in catalog.iter() {
        match module.rollback(&ctx).await {
            Ok(()) => {
                record(module.name(), true, "rolled back");
                println!("rolled back {}.", module.name());
            }
            Err(e) => {
                record(module.name(), false, &e.to_string());
                eprintln!("[-] {}: {e}", module.name());
            }
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
