mod cmd;
mod output;

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use crumbs_core::inventory::{Inventory, SourceStatus};
use crumbs_core::{Env, SourceError};

#[derive(Parser)]
#[command(
    name = "crumbs",
    version,
    about = "Uninstall Linux apps and their leftovers"
)]
struct Cli {
    /// Treat this directory as the filesystem root (testing only)
    #[arg(long, env = "CRUMBS_ROOT", hide = true, global = true)]
    root: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// List and inspect installed apps
    #[command(subcommand)]
    Apps(cmd::apps::AppsCmd),
    /// Check which sources and tools crumbs can use
    Doctor {
        /// Machine-readable output
        #[arg(long)]
        json: bool,
    },
}

/// Some source failed while at least one other source worked.
pub fn partial_failure(inv: &Inventory) -> bool {
    partial_failure_parts(&inv.statuses, &inv.errors)
}

pub fn partial_failure_parts(statuses: &[SourceStatus], errors: &[SourceError]) -> bool {
    !errors.is_empty()
        && statuses.iter().any(|s| {
            s.available
                && s.skipped_reason.is_none()
                && !errors.iter().any(|e| e.source == s.source)
        })
}

fn run(cli: Cli) -> anyhow::Result<ExitCode> {
    let env = match &cli.root {
        Some(root) => Env::fake(root)?,
        None => Env::from_system()?,
    };
    match cli.command {
        Command::Apps(c) => cmd::apps::run(c, &env),
        Command::Doctor { json } => cmd::doctor::run(&env, json),
    }
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::from(1)
        }
    }
}
