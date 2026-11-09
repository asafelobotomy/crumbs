//! `restore`: puts trashed items and edited files back.

use std::io::Write;
use std::process::ExitCode;

use crumbs_core::Env;
use crumbs_core::restore::{RestoreOutcome, restore};
use serde_json::json;

use super::context::{load_cfg, print_notes};
use super::render::{sanitise, shown_path};
use crate::output::{ErrorEntry, print_json_entries};

/// Restores (or with `dry_run` checks) what `selectors` name; also the
/// config warnings.
pub(crate) fn outcomes(
    env: &Env,
    selectors: &[String],
    dry_run: bool,
) -> anyhow::Result<(Vec<RestoreOutcome>, Vec<ErrorEntry>)> {
    let (cfg, notes) = load_cfg(env)?;
    Ok((restore(env, &cfg, selectors, dry_run)?, notes))
}

/// No history entry selected at all (an empty `entry_id` is a selector
/// that matched nothing): a usage error rather than a partial failure.
pub(crate) fn none_selected(outcomes: &[RestoreOutcome]) -> bool {
    !outcomes.is_empty() && outcomes.iter().all(|o| o.entry_id.is_empty() && !o.ok)
}

/// One outcome as the CLI prints it.
pub(crate) fn outcome_line(env: &Env, o: &RestoreOutcome, dry_run: bool) -> String {
    // A selector that matched nothing has no entry; its path is the
    // selector text.
    let path = if o.entry_id.is_empty() {
        sanitise(&o.path.display().to_string())
    } else {
        shown_path(env, &o.path)
    };
    if o.ok && o.entry_id.is_empty() {
        // A batch with nothing to put back: no entry, not an error.
        sanitise(o.error.as_deref().unwrap_or_default())
    } else if o.ok {
        let verb = if dry_run { "would restore" } else { "restored" };
        format!("✓ {verb} {path}")
    } else {
        let err = sanitise(o.error.as_deref().unwrap_or("failed"));
        format!("✗ {path}: {err}")
    }
}

pub fn run(env: &Env, selectors: &[String], dry_run: bool, json: bool) -> anyhow::Result<ExitCode> {
    let (outcomes, notes) = outcomes(env, selectors, dry_run)?;
    let all_ok = outcomes.iter().all(|o| o.ok);
    let none_selected = none_selected(&outcomes);
    if json {
        print_json_entries("restore", &json!({ "outcomes": outcomes }), &notes)?;
    } else {
        let mut out = anstream::stdout().lock();
        for o in &outcomes {
            writeln!(out, "{}", outcome_line(env, o, dry_run))?;
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
