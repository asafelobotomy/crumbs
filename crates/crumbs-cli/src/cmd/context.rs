//! Shared setup for commands that need leftovers: config, rules, a full
//! inventory and the leftover index.

use std::io::Write;

use anyhow::anyhow;
use crumbs_core::command::SystemRunner;
use crumbs_core::config::{Config, load_config};
use crumbs_core::inventory::{Inventory, discover};
use crumbs_core::leftovers::LeftoverIndex;
use crumbs_core::rules::load_rules;
use crumbs_core::sources::ListOptions;
use crumbs_core::{CoreError, Env};

use crate::output::ErrorEntry;

/// Loads `config.toml`. A parse error is fatal; dropped entries come back as
/// `config` notes.
pub fn load_cfg(env: &Env) -> anyhow::Result<(Config, Vec<ErrorEntry>)> {
    let cfg = load_config(env).map_err(|e| match e {
        CoreError::Parse { what, message } => anyhow!("{what}: {message}"),
        other => anyhow!(other),
    })?;
    let notes = cfg
        .warnings
        .iter()
        .map(|m| ErrorEntry::new("config", m.clone()))
        .collect();
    Ok((cfg, notes))
}

pub struct Context {
    pub inv: Inventory,
    pub idx: LeftoverIndex,
    /// Config warnings and rule errors; they never change the exit code.
    pub notes: Vec<ErrorEntry>,
}

impl Context {
    /// Builds the index from an `include_all` inventory so sharing sees
    /// every package.
    pub fn load(env: &Env) -> anyhow::Result<Context> {
        let (cfg, mut notes) = load_cfg(env)?;
        let (rules, rule_errors) = load_rules(env);
        notes.extend(
            rule_errors
                .iter()
                .map(|e| ErrorEntry::new("rules", e.to_string())),
        );
        let inv = discover(env, &SystemRunner, &ListOptions { include_all: true }, &cfg);
        let idx = LeftoverIndex::build(env, &cfg, &rules, &inv);
        Ok(Context { inv, idx, notes })
    }
}

/// Prints notes as `warning: <source>: <message>` on stderr, or for
/// `config` as `warning: <message>`.
pub fn print_notes(notes: &[ErrorEntry]) -> std::io::Result<()> {
    let mut err = anstream::stderr().lock();
    for n in notes {
        if n.source == "config" {
            writeln!(err, "warning: {}", n.message)?;
        } else {
            writeln!(err, "warning: {}: {}", n.source, n.message)?;
        }
    }
    Ok(())
}
