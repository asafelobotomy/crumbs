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
    /// Find reclaimable space: caches, trash, temp files and more (read-only)
    Scan(cmd::scan::ScanArgs),
    /// Free reclaimable space (to the trash by default)
    Clean(cmd::clean::CleanArgs),
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
        Command::Scan(args) => cmd::scan::run(&env, &args),
        Command::Clean(args) => cmd::clean::run(&env, &args),
        Command::Doctor { json } => cmd::doctor::run(&env, json),
    }
}

/// The envelope `command` of an invocation that asked for `--json`.
fn json_command(command: &Command) -> Option<&'static str> {
    match command {
        Command::Apps(cmd::apps::AppsCmd::List { json: true, .. }) => Some("apps list"),
        Command::Apps(cmd::apps::AppsCmd::Show { json: true, .. }) => Some("apps show"),
        Command::Uninstall(a) if a.act.json => Some("uninstall"),
        Command::Orphans(a) if a.act.json => Some("orphans"),
        Command::History { json: true, .. } => Some("history"),
        Command::Restore { json: true, .. } => Some("restore"),
        Command::Scan(a) if a.json => Some("scan"),
        Command::Clean(a) if a.act.json => Some("clean"),
        Command::Doctor { json: true } => Some("doctor"),
        _ => None,
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let json_command = json_command(&cli.command);
    match run(cli) {
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
            if let Some(command) = json_command {
                let source = if e.is::<cmd::context::ConfigError>() {
                    "config"
                } else {
                    "cli"
                };
                let entry = output::ErrorEntry::new(source, format!("{e:#}"));
                if output::print_json_entries(command, &serde_json::Value::Null, &[entry]).is_err()
                {
                    return ExitCode::from(1);
                }
            }
            ExitCode::from(1)
        }
    }
}
