//! `restore`: puts trashed items and edited files back.

use std::io::Write;
use std::process::ExitCode;

use crumbs_core::Env;
use crumbs_core::env::display_path;
use crumbs_core::restore::restore;
use serde_json::json;

use super::context::{load_cfg, print_notes};
use crate::output::print_json_entries;

pub fn run(env: &Env, selectors: &[String], dry_run: bool, json: bool) -> anyhow::Result<ExitCode> {
    let (cfg, notes) = load_cfg(env)?;
    let outcomes = restore(env, &cfg, selectors, dry_run)?;
    let all_ok = outcomes.iter().all(|o| o.ok);
    // No history entry selected at all (an empty `entry_id` is a selector
    // that matched nothing): a usage error rather than a partial failure.
    let none_selected = !outcomes.is_empty() && outcomes.iter().all(|o| o.entry_id.is_empty());
    if json {
        print_json_entries("restore", &json!({ "outcomes": outcomes }), &notes)?;
    } else {
        let mut out = anstream::stdout().lock();
        for o in &outcomes {
            // A selector that matched nothing has no entry; its path is the
            // selector text.
            let path = if o.entry_id.is_empty() {
                o.path.display().to_string()
            } else {
                display_path(env, &o.path)
            };
            if o.ok {
                let verb = if dry_run { "would restore" } else { "restored" };
                writeln!(out, "✓ {verb} {path}")?;
            } else {
                let err = o.error.as_deref().unwrap_or("failed");
                writeln!(out, "✗ {path}: {err}")?;
            }
        }
        drop(out);
        print_notes(&notes)?;
    }
    Ok(ExitCode::from(if none_selected {
        2
    } else if all_ok {
        0
    } else {
        3
    }))
}
