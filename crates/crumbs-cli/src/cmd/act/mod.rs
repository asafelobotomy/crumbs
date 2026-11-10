//! The flow shared by the commands that change files (`uninstall`,
//! `orphans --clean`, `clean`): usage checks, choosing findings, then
//! preview, running-app check, confirmation, execution and report.

use std::io::Write;
use std::path::Path;
use std::process::ExitCode;

use clap::Args;
use crumbs_core::command::{SystemRunner, USER_COMMAND_TIMEOUT};
use crumbs_core::config::Config;
use crumbs_core::executor::{ExecReport, execute_many, running_processes};
use crumbs_core::inventory::Inventory;
use crumbs_core::leftovers::{Finding, FindingKind};
use crumbs_core::plan::{Action, Plan, PlannedAction};
use crumbs_core::removal::still_installed;
use crumbs_core::safety::Scope;
use crumbs_core::{App, Env, SourceKind};
use serde_json::json;

use super::build::{Refusal, Stop};
use super::context::{notes_text, print_notes};
use super::manual::{is_privileged, manual_commands};
use super::render;
use crate::output::{ErrorEntry, finding_kind, print_json_entries};
use crate::prompt;

mod execute;
mod preview;
mod privilege;
mod select;
mod signals;
#[cfg(debug_assertions)]
mod test_runner;
pub use execute::*;
pub use preview::*;
pub use select::*;

pub(crate) use privilege::Privilege;
pub use privilege::usable;

/// Flags shared by the commands that change files.
#[derive(Args, Debug, Clone, Default)]
pub struct ActFlags {
    /// Choose the items in a checklist
    #[arg(short = 'i', long)]
    pub interactive: bool,
    /// Show the plan and stop
    #[arg(long)]
    pub dry_run: bool,
    /// Do not ask for confirmation
    #[arg(long)]
    pub yes: bool,
    /// Delete instead of moving to the trash (cannot be undone)
    #[arg(long)]
    pub permanent: bool,
    /// Continue despite failed sources or a running app
    #[arg(long)]
    pub force: bool,
    /// Machine-readable output (needs --yes or --dry-run)
    #[arg(long)]
    pub json: bool,
}

pub const NON_INTERACTIVE: &str = "refusing to act without --yes in a non-interactive session";
pub const NO_RIGHTS: &str = "no way to get administrator rights";
pub const NOT_GRANTED: &str =
    "administrator rights were not granted; nothing that needs them was changed";
pub const BATCH_STOPPED: &str = "the privileged run stopped before it finished; \
     the step it was in may have partly run, and the steps after it did not run";
/// Exit code when administrator rights were needed but not available.
pub const EXIT_NO_RIGHTS: u8 = 5;
/// How a tree delete's manual command reads (see `manual::root_command`).
pub const TREE_RM_WORDS: &str = "'-rf' '--one-file-system'";
/// After manual commands with a tree delete (human output only): the
/// batch's own mount checks do not run by hand.
pub const NOTE_CHECK_MOUNTS: &str =
    "check that nothing is mounted inside these paths before running the rm commands";

/// Where a command's refusals go: always stderr, and under `--json` also
/// an envelope on stdout with a `cli` error entry.
#[derive(Clone, Copy)]
pub struct Out<'a> {
    pub command: &'a str,
    pub json: bool,
}

impl Out<'_> {
    /// Prints `error: <msg>` and the indented `details`, and returns `code`.
    pub fn fail_with(&self, code: u8, msg: &str, details: &[String]) -> anyhow::Result<ExitCode> {
        self.fail_after(code, msg, details, &[])
    }

    /// [`Self::fail_with`] after the entries that caused it: they are
    /// printed as warnings first and lead the envelope's `errors`.
    fn fail_after(
        &self,
        code: u8,
        msg: &str,
        details: &[String],
        causes: &[ErrorEntry],
    ) -> anyhow::Result<ExitCode> {
        print_notes(causes)?;
        let mut err = anstream::stderr().lock();
        writeln!(err, "error: {}", render::sanitise(msg))?;
        for d in details {
            writeln!(err, "  {}", render::sanitise(d))?;
        }
        drop(err);
        self.envelope(msg, details, causes)?;
        Ok(ExitCode::from(code))
    }

    /// Prints `error: <msg>` and the commands to run by hand, one per
    /// line; under `--json` the envelope's `data` also holds them as
    /// `manual_commands`. Returns exit 5.
    fn no_rights(
        &self,
        msg: &str,
        manual: &[String],
        mut data: serde_json::Value,
        warnings: &[ErrorEntry],
    ) -> anyhow::Result<ExitCode> {
        let mut err = anstream::stderr().lock();
        writeln!(err, "error: {msg}")?;
        if !manual.is_empty() {
            writeln!(err, "to do it yourself, run:")?;
        }
        for m in manual {
            writeln!(err, "  {m}")?;
        }
        if manual.iter().any(|m| m.contains(TREE_RM_WORDS)) {
            writeln!(err, "{NOTE_CHECK_MOUNTS}")?;
        }
        drop(err);
        if self.json {
            data["manual_commands"] = json!(manual);
            let mut errors = warnings.to_vec();
            errors.push(ErrorEntry::new("cli", msg.to_string()));
            print_json_entries(self.command, &data, &errors)?;
        }
        Ok(ExitCode::from(EXIT_NO_RIGHTS))
    }

    pub fn fail(&self, code: u8, msg: &str) -> anyhow::Result<ExitCode> {
        self.fail_with(code, msg, &[])
    }

    /// The user said no (exit 4).
    pub fn aborted(&self) -> anyhow::Result<ExitCode> {
        writeln!(anstream::stderr().lock(), "aborted")?;
        self.envelope("aborted", &[], &[])?;
        Ok(ExitCode::from(4))
    }

    fn envelope(&self, msg: &str, details: &[String], causes: &[ErrorEntry]) -> anyhow::Result<()> {
        if !self.json {
            return Ok(());
        }
        let message = if details.is_empty() {
            msg.to_string()
        } else {
            format!("{msg} {}", details.join("; "))
        };
        let mut errors = causes.to_vec();
        errors.push(ErrorEntry::new("cli", message));
        print_json_entries(
            self.command,
            &json!({ "plans": [], "report": null }),
            &errors,
        )
    }
}

/// Usage checks that need no scan. `Some(code)` means stop.
pub fn precheck(out: Out, act: &ActFlags) -> anyhow::Result<Option<ExitCode>> {
    if act.json && !act.yes && !act.dry_run {
        return out.fail(2, "--json needs --yes or --dry-run").map(Some);
    }
    let interactive = prompt::is_interactive();
    if act.interactive && !interactive {
        return out.fail(2, "-i needs an interactive terminal").map(Some);
    }
    if !act.dry_run && !act.yes && !interactive {
        return out.fail(2, NON_INTERACTIVE).map(Some);
    }
    Ok(None)
}

/// Steps 4 to 7: preview, running-app check, confirmation, execution and
/// report. `errors` (failed sources, then notes) are warnings in the
/// preview and the envelope's `errors` under `--json`. JSON `report` has
/// one entry per plan, in order: `null` for a plan with nothing to do; the
/// whole field is `null` on a dry run. `after_preview` are extra `note:`
/// lines for the human preview only (never in the JSON).
pub fn apply(
    env: &Env,
    cfg: &Config,
    errors: &[ErrorEntry],
    out: Out,
    targets: &[Target],
    act: &ActFlags,
    after_preview: &[String],
) -> anyhow::Result<ExitCode> {
    let plans: Vec<&Plan> = targets.iter().map(|t| &t.plan).collect();
    let empty = plans.iter().all(|p| p.actions.is_empty());

    if !act.json {
        print_preview(env, targets, after_preview, errors)?;
    }
    if act.dry_run {
        if act.json {
            print_json_entries(out.command, &dry_run_data(targets), errors)?;
        }
        return Ok(ExitCode::SUCCESS);
    }
    if empty {
        if act.json {
            let nulls = vec![serde_json::Value::Null; plans.len()];
            let data = json!({ "plans": plans, "report": nulls });
            print_json_entries(out.command, &data, errors)?;
        } else {
            writeln!(anstream::stdout().lock(), "nothing to do")?;
        }
        return Ok(ExitCode::SUCCESS);
    }

    let needs_rights = plans.iter().flat_map(|p| &p.actions).any(is_privileged);
    let privilege = Privilege::for_run(env, needs_rights);
    if needs_rights && privilege.esc.is_none() {
        let manual = manual_commands(env, plans.iter().flat_map(|p| &p.actions));
        let data = json!({ "plans": plans, "report": null });
        return out.no_rights(NO_RIGHTS, &manual, data, errors);
    }

    let interactive = prompt::is_interactive();
    // The apps the user said to go on with although they are running.
    let mut confirmed: Vec<String> = Vec::new();
    for (app, msg) in running_apps(env, targets) {
        writeln!(anstream::stderr().lock(), "{msg}")?;
        if act.force {
            continue;
        }
        if !interactive {
            return out.fail(1, &running_refusal(&msg));
        }
        if !prompt::confirm(&format!(
            "{} is running; continue anyway?",
            render::sanitise(&app.display_name)
        ))? {
            return out.aborted();
        }
        confirmed.push(app.id.clone());
    }
    if !act.yes {
        if !interactive {
            return out.fail(2, NON_INTERACTIVE);
        }
        if !prompt::confirm("Proceed?")? {
            return out.aborted();
        }
    }

    let allowed = if act.force {
        AllowedRunning::Any
    } else if confirmed.is_empty() {
        AllowedRunning::None
    } else {
        AllowedRunning::Only(confirmed)
    };
    let outcome = execute_confirmed(env, cfg, targets, privilege, allowed);
    if let Some(msg) = &outcome.refusal {
        return out.fail(2, msg);
    }
    if let Some(msg) = outcome.running.first()
        && outcome.reports.is_empty()
    {
        // An app started after the check above: nothing ran.
        writeln!(anstream::stderr().lock(), "{msg}")?;
        return out.fail(1, &running_refusal(msg));
    }
    let reports = &outcome.reports;
    let not_granted = reports.iter().flatten().any(|r| r.escalation_failed);
    // The root shell ran and was stopped part-way: some privileged steps
    // ran, so this is not "nothing changed"; the results say which.
    let stopped = reports.iter().flatten().any(|r| r.batch_stopped);
    if act.json {
        let data = json!({ "plans": plans, "report": reports });
        if not_granted {
            return out.no_rights(NOT_GRANTED, &outcome.manual_commands, data, errors);
        }
        if stopped {
            let mut all = errors.to_vec();
            all.push(ErrorEntry::new("cli", BATCH_STOPPED.to_string()));
            writeln!(anstream::stderr().lock(), "error: {BATCH_STOPPED}")?;
            print_json_entries(out.command, &data, &all)?;
            return Ok(ExitCode::from(outcome.exit_code));
        }
        print_json_entries(out.command, &data, errors)?;
    } else {
        writeln!(anstream::stdout().lock())?;
        let runs: Vec<(&Plan, &ExecReport)> = plans
            .iter()
            .zip(reports)
            .filter_map(|(p, r)| r.as_ref().map(|r| (*p, r)))
            .collect();
        render::report(env, &runs)?;
        if not_granted {
            return out.no_rights(
                NOT_GRANTED,
                &outcome.manual_commands,
                serde_json::Value::Null,
                errors,
            );
        }
        if stopped {
            writeln!(anstream::stderr().lock(), "error: {BATCH_STOPPED}")?;
            return Ok(ExitCode::from(outcome.exit_code));
        }
    }
    Ok(ExitCode::from(outcome.exit_code))
}

#[cfg(test)]
mod tests;
