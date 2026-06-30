//! vpsguard CLI entry point.

use std::path::PathBuf;

use clap::{Parser, Subcommand};
use color_eyre::eyre::{bail, eyre, Context as _, Result};
use vpsguard_agent::Auth;
use vpsguard_core::{Config, Context, Inventory, ModuleCatalog, State};
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

    /// Run against a remote host over SSH (user@host[:port]).
    #[arg(long, global = true)]
    target: Option<String>,

    /// Run against all inventory servers carrying this tag.
    #[arg(long, global = true)]
    group: Option<String>,

    /// Inventory file used by --group.
    #[arg(long, global = true, default_value = "inventory.toml")]
    inventory: PathBuf,

    /// Prompt for the SSH password (otherwise read $VPSGUARD_SSH_PASSWORD).
    #[arg(long, global = true)]
    ask_pass: bool,

    #[command(subcommand)]
    command: Command,
}

/// A resolved remote host to act on.
struct Remote {
    host: String,
    port: u16,
    user: String,
}

fn parse_target(s: &str) -> Result<Remote> {
    let (user, rest) = s
        .split_once('@')
        .ok_or_else(|| eyre!("target must be user@host[:port]"))?;
    let (host, port) = match rest.rsplit_once(':') {
        Some((h, p)) => (h, p.parse().map_err(|_| eyre!("bad port in target"))?),
        None => (rest, 22u16),
    };
    Ok(Remote {
        host: host.to_string(),
        port,
        user: user.to_string(),
    })
}

/// Resolve the hosts to act on from --target / --group; empty means local.
fn resolve_remotes(cli: &Cli) -> Result<Vec<Remote>> {
    if let Some(t) = &cli.target {
        return Ok(vec![parse_target(t)?]);
    }
    if let Some(group) = &cli.group {
        let raw = std::fs::read_to_string(&cli.inventory)
            .with_context(|| format!("reading inventory {}", cli.inventory.display()))?;
        let inv = Inventory::from_toml(&raw)?;
        let selected = inv.select(group);
        if selected.is_empty() {
            bail!(
                "no servers match group `{group}` in {}",
                cli.inventory.display()
            );
        }
        return Ok(selected
            .into_iter()
            .map(|(_name, s)| Remote {
                host: s.host.clone(),
                port: s.port,
                user: s.user.clone(),
            })
            .collect());
    }
    Ok(Vec::new())
}

fn ssh_auth(ask_pass: bool) -> Result<Auth> {
    if ask_pass {
        let p = inquire::Password::new("SSH password:")
            .without_confirmation()
            .prompt()?;
        return Ok(Auth::Password(p));
    }
    match std::env::var("VPSGUARD_SSH_PASSWORD") {
        Ok(p) => Ok(Auth::Password(p)),
        Err(_) => bail!("set $VPSGUARD_SSH_PASSWORD or pass --ask-pass for remote auth"),
    }
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
    match &cli.command {
        Command::Init { force } => cmd_init(&cli.config, *force),
        Command::Plan => cmd_plan(&cli).await,
        Command::Apply {
            dry_run,
            yes,
            no_guard,
        } => cmd_apply(&cli, *dry_run, *yes, *no_guard).await,
        Command::Audit => cmd_audit(&cli).await,
        Command::Rollback { module } => cmd_rollback(&cli.config, module.as_deref()).await,
        Command::Recipes => {
            cmd_recipes();
            Ok(())
        }
        Command::History { limit } => {
            cmd_history(*limit);
            Ok(())
        }
        Command::Guard {
            modules,
            timeout,
            commit,
        } => cmd_guard(&cli.config, modules, *timeout, commit).await,
    }
}

/// Build the contexts to act on: one local, or one per resolved remote host.
async fn contexts(cli: &Cli) -> Result<Vec<(String, Context)>> {
    let config = load_config(&cli.config)?;
    let remotes = resolve_remotes(cli)?;
    if remotes.is_empty() {
        return Ok(vec![("local".to_string(), Context::system(config))]);
    }
    let auth = ssh_auth(cli.ask_pass)?;
    let mut out = Vec::new();
    for r in remotes {
        let ctx = vpsguard_agent::remote_context(config.clone(), &r.host, r.port, &r.user, &auth)
            .await
            .map_err(|e| eyre!("{e}"))?;
        out.push((format!("{}@{}:{}", r.user, r.host, r.port), ctx));
    }
    Ok(out)
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

fn load_config(config_path: &PathBuf) -> Result<Config> {
    let raw = std::fs::read_to_string(config_path)
        .with_context(|| format!("reading config {}", config_path.display()))?;
    Config::resolve(&raw).with_context(|| format!("parsing {}", config_path.display()))
}

fn load(config_path: &PathBuf) -> Result<(Context, ModuleCatalog)> {
    Ok((
        Context::system(load_config(config_path)?),
        vpsguard_modules::catalog(),
    ))
}

async fn run_plan(ctx: &Context, catalog: &ModuleCatalog) -> Result<()> {
    let mut total = 0;
    for module in catalog.iter() {
        let status = module.check(ctx).await?;
        let changes = pending(module, ctx, &status).await?;
        total += changes.len();
        render::module_plan(module, &status, &changes);
    }
    render::summary(total);
    Ok(())
}

async fn run_audit(ctx: &Context, catalog: &ModuleCatalog) -> Result<()> {
    let mut compliant = 0;
    let mut applicable = 0;
    for module in catalog.iter() {
        let status = module.check(ctx).await?;
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

async fn cmd_plan(cli: &Cli) -> Result<()> {
    let catalog = vpsguard_modules::catalog();
    let remote = cli.target.is_some() || cli.group.is_some();
    for (label, ctx) in contexts(cli).await? {
        if remote {
            println!("=== {label} ===");
        }
        run_plan(&ctx, &catalog).await?;
        if remote {
            println!();
        }
    }
    Ok(())
}

async fn cmd_apply(cli: &Cli, dry_run: bool, yes: bool, no_guard: bool) -> Result<()> {
    if cli.target.is_some() || cli.group.is_some() {
        bail!(
            "remote apply is not yet supported; use `plan`/`audit` with --target, or apply locally"
        );
    }
    let config_path = &cli.config;
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

async fn cmd_audit(cli: &Cli) -> Result<()> {
    let catalog = vpsguard_modules::catalog();
    let remote = cli.target.is_some() || cli.group.is_some();
    for (label, ctx) in contexts(cli).await? {
        if remote {
            println!("=== {label} ===");
        }
        run_audit(&ctx, &catalog).await?;
        if remote {
            println!();
        }
    }
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
