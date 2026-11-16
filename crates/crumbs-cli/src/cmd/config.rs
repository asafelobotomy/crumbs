//! `crumbs config`: show and change `~/.config/crumbs/config.toml` one key at a time.

use std::io::Write;
use std::process::ExitCode;

use clap::Subcommand;
use crumbs_core::Env;
use crumbs_core::config::Config;
use crumbs_core::config::edit::{Edit, KEYS, apply_edit, config_path};
use serde_json::{Value, json};

use super::context::{ConfigError, load_cfg, print_notes};
use super::render::sanitise;
use crate::output::print_json_entries;

#[derive(Subcommand, Debug)]
pub enum ConfigCmd {
    /// Show every setting in effect, with the file it comes from
    Show {
        /// Machine-readable output
        #[arg(long)]
        json: bool,
    },
    /// Set a number or size: temp_age_days, log_age_days, build_age_days,
    /// snapshots_keep, pkg_cache_keep or journal_max
    Set {
        key: String,
        value: String,
        /// Machine-readable output
        #[arg(long)]
        json: bool,
    },
    /// Add an entry to a list: protect, exclude, appimage_dirs, project_dirs,
    /// categories_off or categories_on
    Add {
        key: String,
        value: String,
        /// Machine-readable output
        #[arg(long)]
        json: bool,
    },
    /// Take an entry out of a list
    Remove {
        key: String,
        value: String,
        /// Machine-readable output
        #[arg(long)]
        json: bool,
    },
    /// Go back to the default for a key, or for every key with `all`
    Reset {
        key: String,
        /// Machine-readable output
        #[arg(long)]
        json: bool,
    },
}

impl ConfigCmd {
    pub fn json(&self) -> bool {
        match self {
            ConfigCmd::Show { json }
            | ConfigCmd::Set { json, .. }
            | ConfigCmd::Add { json, .. }
            | ConfigCmd::Remove { json, .. }
            | ConfigCmd::Reset { json, .. } => *json,
        }
    }

    pub fn json_name(&self) -> &'static str {
        match self {
            ConfigCmd::Show { .. } => "config show",
            ConfigCmd::Set { .. } => "config set",
            ConfigCmd::Add { .. } => "config add",
            ConfigCmd::Remove { .. } => "config remove",
            ConfigCmd::Reset { .. } => "config reset",
        }
    }
}

/// The settings in effect, as `config show --json` gives them.
pub fn values(env: &Env, cfg: &Config) -> Value {
    let paths = |v: &[std::path::PathBuf]| -> Vec<String> {
        v.iter()
            .map(|p| crumbs_core::env::display_path(env, p))
            .collect()
    };
    let cats = |v: &[crumbs_core::bloat::Category]| -> Vec<&str> {
        v.iter().map(|c| c.as_str()).collect()
    };
    json!({
        "protect": paths(&cfg.protect),
        "exclude": cfg.exclude.iter().map(|p| crumbs_core::env::display_path(env, std::path::Path::new(p))).collect::<Vec<_>>(),
        "appimage_dirs": paths(&cfg.appimage_dirs),
        "project_dirs": paths(&cfg.project_dirs),
        "categories_off": cats(&cfg.categories_off),
        "categories_on": cats(&cfg.categories_on),
        "temp_age_days": cfg.temp_age_days,
        "log_age_days": cfg.log_age_days,
        "build_age_days": cfg.build_age_days,
        "snapshots_keep": cfg.snapshots_keep,
        "pkg_cache_keep": cfg.pkg_cache_keep,
        "journal_max": cfg.journal_max,
    })
}

fn data(env: &Env, cfg: &Config, changed: Option<bool>) -> Value {
    let keys: Vec<Value> = KEYS
        .iter()
        .map(|(k, kind)| json!({ "key": k, "kind": kind }))
        .collect();
    let mut d = json!({
        "path": crumbs_core::env::display_path(env, &env.unsys(&config_path(env))),
        "values": values(env, cfg),
        "keys": keys,
    });
    if let Some(c) = changed {
        d["changed"] = json!(c);
    }
    d
}

pub fn run(cmd: ConfigCmd, env: &Env) -> anyhow::Result<ExitCode> {
    let json = cmd.json();
    let name = cmd.json_name();
    let edit = match cmd {
        ConfigCmd::Show { .. } => None,
        ConfigCmd::Set { key, value, .. } => Some(Edit::Set { key, value }),
        ConfigCmd::Add { key, value, .. } => Some(Edit::Add { key, value }),
        ConfigCmd::Remove { key, value, .. } => Some(Edit::Remove { key, value }),
        ConfigCmd::Reset { key, .. } => Some(Edit::Reset { key }),
    };
    let changed = match &edit {
        None => None,
        Some(e) => Some(apply_edit(env, e).map_err(|e| ConfigError(e.to_string()))?),
    };
    let (cfg, notes) = load_cfg(env)?;
    if json {
        print_json_entries(name, &data(env, &cfg, changed), &notes)?;
        return Ok(ExitCode::SUCCESS);
    }
    let mut out = anstream::stdout().lock();
    match changed {
        Some(true) => writeln!(out, "Saved.")?,
        Some(false) => writeln!(out, "Nothing to change.")?,
        None => {}
    }
    if changed.is_none() {
        writeln!(
            out,
            "# {}",
            sanitise(&crumbs_core::env::display_path(
                env,
                &env.unsys(&config_path(env))
            ))
        )?;
        if let Value::Object(map) = values(env, &cfg) {
            for (k, v) in map {
                writeln!(out, "{k} = {}", sanitise(&v.to_string()))?;
            }
        }
    }
    drop(out);
    print_notes(&notes)?;
    Ok(ExitCode::SUCCESS)
}
