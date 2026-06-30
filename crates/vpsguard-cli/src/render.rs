//! Plain-text rendering of plans for the CLI. The TUI renders separately.

use vpsguard_core::{Change, Module, State, Status};

/// Print a module header, its status, and any pending changes.
pub fn module_plan(module: &dyn Module, status: &Status, changes: &[Change]) {
    let marker = match status.state {
        State::Compliant => "ok",
        State::Drift => "~",
        State::Error => "x",
        State::NotApplicable => "-",
    };
    println!("[{marker}] {} ({})", module.name(), module.summary());

    match status.state {
        State::Compliant => println!("    compliant"),
        State::NotApplicable => println!("    n/a: {}", status.detail),
        State::Error => println!("    error: {}", status.detail),
        State::Drift => {
            for c in changes {
                match (&c.before, &c.after) {
                    (Some(b), Some(a)) => println!("    ~ {}: {b} -> {a}", strip_arrow(&c.summary)),
                    _ => println!("    + {}", c.summary),
                }
            }
        }
    }
}

/// Trailing summary line.
pub fn summary(total_changes: usize) {
    println!();
    match total_changes {
        0 => println!("Nothing to do; system matches config."),
        1 => println!("1 change pending. Run `vpsguard apply` to converge."),
        n => println!("{n} changes pending. Run `vpsguard apply` to converge."),
    }
}

/// The module already embeds `key: before -> after`; show only the key here.
fn strip_arrow(summary: &str) -> &str {
    summary.split(':').next().unwrap_or(summary)
}
