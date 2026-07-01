//! vpsguard CLI entry point.

use std::path::PathBuf;

use clap::{Parser, Subcommand};
use color_eyre::eyre::{bail, eyre, Context as _, Result};
use vpsguard_agent::{default_known_hosts, Auth, ConnectOpts, HostKeyPolicy};
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

    /// SSH private key file for remote auth.
    #[arg(short = 'i', long, global = true)]
    identity: Option<PathBuf>,

    /// known_hosts file for host-key verification.
    #[arg(long, global = true)]
    known_hosts: Option<PathBuf>,

    /// Only accept host keys already in known_hosts.
    #[arg(long, global = true)]
    strict_host_key: bool,

    /// Accept any host key without checking (insecure; testing only).
    #[arg(long, global = true)]
    insecure_host_key: bool,

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

/// Pick the first existing default SSH key.
fn default_key() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    ["id_ed25519", "id_rsa"]
        .iter()
        .map(|n| PathBuf::from(&home).join(".ssh").join(n))
        .find(|p| p.exists())
}

/// Resolve SSH connection options: auth (identity > password > default key) and
/// host-key policy (TOFU by default).
fn connect_opts(cli: &Cli) -> Result<ConnectOpts> {
    let auth = if let Some(id) = &cli.identity {
        Auth::Key {
            path: id.clone(),
            passphrase: None,
        }
    } else if cli.ask_pass {
        Auth::Password(
            inquire::Password::new("SSH password:")
                .without_confirmation()
                .prompt()?,
        )
    } else if let Ok(p) = std::env::var("VPSGUARD_SSH_PASSWORD") {
        Auth::Password(p)
    } else if let Some(key) = default_key() {
        Auth::Key {
            path: key,
            passphrase: None,
        }
    } else {
        bail!("no SSH auth: pass --identity <key>, --ask-pass, set $VPSGUARD_SSH_PASSWORD, or have a default key");
    };

    let host_key = if cli.insecure_host_key {
        HostKeyPolicy::AcceptAny
    } else if cli.strict_host_key {
        HostKeyPolicy::Strict
    } else {
        HostKeyPolicy::Tofu
    };

    Ok(ConnectOpts {
        auth,
        host_key,
        known_hosts: cli.known_hosts.clone().unwrap_or_else(default_known_hosts),
    })
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
    /// Remove installed modules: stop services, remove packages, delete config.
    Uninstall {
        /// Limit to one module by name (default: all, provisioning first).
        module: Option<String>,
        /// Skip the confirmation prompt.
        #[arg(long)]
        yes: bool,
        /// Also delete data (databases, swap file, app checkout).
        #[arg(long)]
        purge: bool,
    },
    /// Launch the interactive dashboard.
    Tui,
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
        Command::Uninstall { module, yes, purge } => {
            cmd_uninstall(&cli, module.as_deref(), *yes, *purge).await
        }
        Command::Tui => {
            let config = load_config(&cli.config)?;
            vpsguard_tui::run(config).await.map_err(|e| eyre!("{e}"))
        }
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
    let opts = connect_opts(cli)?;
    let mut out = Vec::new();
    for r in remotes {
        let ctx = vpsguard_agent::remote_context(config.clone(), &r.host, r.port, &r.user, &opts)
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

async fn cmd_uninstall(cli: &Cli, only: Option<&str>, yes: bool, purge: bool) -> Result<()> {
    let ctxs = contexts(cli).await?;
    let scope = only.unwrap_or("all modules");
    let data = if purge { " and DELETE their data" } else { "" };
    println!("This will uninstall {scope}{data}.");
    if !yes {
        let msg = if purge {
            "Uninstall and purge data? This is destructive"
        } else {
            "Uninstall these modules?"
        };
        if !inquire::Confirm::new(msg).with_default(false).prompt()? {
            println!("aborted.");
            return Ok(());
        }
    }

    let history = open_history();
    let remote = ctxs.len() > 1 || ctxs.first().is_some_and(|(l, _)| l != "local");
    for (label, ctx) in &ctxs {
        if remote {
            println!("\n== {label} ==");
        }
        let catalog = vpsguard_modules::catalog();
        // Uninstall provisioning before the security baseline (reverse of apply).
        let mut modules: Vec<_> = catalog.iter().collect();
        modules.reverse();
        for module in modules {
            if only.is_some_and(|n| n != module.name()) {
                continue;
            }
            match module.uninstall(ctx, purge).await {
                Ok(report) => {
                    render::apply_report(&report);
                    if let Some(h) = &history {
                        let _ = h.record("uninstall", module.name(), "removed", true);
                    }
                }
                Err(e) => {
                    if let Some(h) = &history {
                        let _ = h.record("uninstall", module.name(), &e.to_string(), false);
                    }
                    eprintln!("[x] {}: {e}", module.name());
                }
            }
        }
    }

    if let Some(name) = only {
        if vpsguard_modules::catalog().get(name).is_none() {
            bail!("unknown module `{name}`");
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
