mod cmd;
mod output;
mod prompt;

#[cfg(debug_assertions)]
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use crumbs_core::Env;

#[derive(Parser)]
#[command(
    name = "crumbs",
    version,
    about = "Uninstall Linux apps and their leftovers"
)]
struct Cli {
    /// Treat this directory as the filesystem root (debug builds only)
    #[cfg(debug_assertions)]
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
    /// Remove apps and their leftovers (to the trash by default)
    Uninstall(cmd::uninstall::UninstallArgs),
    /// List per-user files that belong to no installed app, or clean them
    Orphans(cmd::orphans::OrphansArgs),
    /// Show past removals, newest first
    History {
        /// Number of batches to show
        #[arg(long, default_value_t = 20)]
        limit: usize,
        /// Machine-readable output
        #[arg(long)]
        json: bool,
    },
    /// Undo removals: a batch id, an entry id or an original path
    Restore {
        /// Batch ids, entry ids or absolute original paths
        #[arg(required = true)]
        selectors: Vec<String>,
        /// Check what would be restored and stop
        #[arg(long)]
        dry_run: bool,
        /// Machine-readable output
        #[arg(long)]
        json: bool,
    },
    /// Check which sources and tools crumbs can use
    Doctor {
        /// Machine-readable output
        #[arg(long)]
        json: bool,
    },
}

fn run(cli: Cli) -> anyhow::Result<ExitCode> {
    #[cfg(debug_assertions)]
    let env = match &cli.root {
        Some(root) => Env::fake(root)?,
        None => Env::from_system()?,
    };
    #[cfg(not(debug_assertions))]
    let env = Env::from_system()?;
    match cli.command {
        Command::Apps(c) => cmd::apps::run(c, &env),
        Command::Uninstall(args) => cmd::uninstall::run(&env, &args),
        Command::Orphans(args) => cmd::orphans::run(&env, &args),
        Command::History { limit, json } => cmd::history::run(&env, limit, json),
        Command::Restore {
            selectors,
            dry_run,
            json,
        } => cmd::restore::run(&env, &selectors, dry_run, json),
        Command::Doctor { json } => cmd::doctor::run(&env, json),
    }
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(code) => code,
        Err(e) => {
            let broken_pipe = e.chain().any(|c| {
                c.downcast_ref::<std::io::Error>()
                    .is_some_and(|io| io.kind() == std::io::ErrorKind::BrokenPipe)
            });
            if broken_pipe {
                return ExitCode::SUCCESS;
            }
            eprintln!("error: {e:#}");
            ExitCode::from(1)
        }
    }
}
