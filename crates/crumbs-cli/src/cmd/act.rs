//! The flow shared by the commands that change files (`uninstall`,
//! `orphans --clean`, `clean`): usage checks, choosing findings, then
//! preview, running-app check, confirmation, execution and report.

use std::io::Write;
use std::path::Path;
use std::process::ExitCode;

use clap::Args;
use crumbs_core::config::Config;
use crumbs_core::executor::{ExecReport, execute, running_processes};
use crumbs_core::inventory::Inventory;
use crumbs_core::leftovers::{Finding, FindingKind};
use crumbs_core::plan::{Action, Plan, PlannedAction};
use crumbs_core::{App, Env};
use serde_json::json;

use super::context::print_notes;
use super::render;
use crate::output::{ErrorEntry, finding_kind, fmt_size, print_json_entries};
use crate::prompt;

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

/// Exit 1 when some source failed, unless `--force` or `--dry-run` (a
/// preview only warns).
pub fn refuse_source_errors(
    out: Out,
    inv: &Inventory,
    act: &ActFlags,
) -> anyhow::Result<Option<ExitCode>> {
    let failures: Vec<ErrorEntry> = inv.errors.iter().map(ErrorEntry::from).collect();
    refuse_failed(out, &failures, act)
}

/// [`refuse_source_errors`] for any failures, one entry per failed source:
/// their messages are shown (and under `--json` included) before the
/// refusal.
pub fn refuse_failed(
    out: Out,
    failures: &[ErrorEntry],
    act: &ActFlags,
) -> anyhow::Result<Option<ExitCode>> {
    if failures.is_empty() || act.force || act.dry_run {
        return Ok(None);
    }
    let mut failed: Vec<&str> = failures.iter().map(|e| e.source.as_str()).collect();
    failed.sort_unstable();
    failed.dedup();
    let msg = format!(
        "refusing: some sources failed ({}); re-run with --force to continue",
        failed.join(", ")
    );
    out.fail_after(1, &msg, &[], failures).map(Some)
}

/// Findings a plan may ever contain: a shared `mimeapps.list` edit is
/// never offered.
pub fn plannable(findings: Vec<Finding>) -> Vec<Finding> {
    findings
        .into_iter()
        .filter(|f| !(f.shared && f.kind == FindingKind::MimeAssoc))
        .collect()
}

/// Returns the findings to plan: the default ones, or with `-i` the ones
/// the user ticks. `None` when the checklist was cancelled.
pub fn choose(
    env: &Env,
    prompt_text: &str,
    findings: Vec<Finding>,
    interactive: bool,
    defaults: impl Fn(&Finding) -> bool,
) -> anyhow::Result<Option<Vec<Finding>>> {
    if !interactive {
        return Ok(Some(findings.into_iter().filter(|f| defaults(f)).collect()));
    }
    if findings.is_empty() {
        return Ok(Some(findings));
    }
    let items: Vec<String> = findings
        .iter()
        .map(|f| {
            let shared = if f.shared { "  (shared)" } else { "" };
            format!(
                "{:<12} {:>10}  {}{shared}",
                finding_kind(f.kind),
                fmt_size(f.size.bytes),
                render::shown_path(env, &f.path)
            )
        })
        .collect();
    let ticks: Vec<bool> = findings.iter().map(&defaults).collect();
    let Some(picked) = prompt::multi_select(prompt_text, &items, &ticks)? else {
        return Ok(None);
    };
    Ok(Some(
        findings
            .into_iter()
            .enumerate()
            .filter(|(i, _)| picked.contains(i))
            .map(|(_, f)| f)
            .collect(),
    ))
}

pub struct Target<'a> {
    pub heading: String,
    /// `None` for orphans.
    pub app: Option<&'a App>,
    pub plan: Plan,
}

/// Makes every path planned once: an action whose path equals or lies
/// under a path an earlier target already plans is dropped. A second edit
/// of the same file is merged into the earlier edit instead, since the
/// first edit replaces the file and the second could never verify.
pub fn drop_overlaps(targets: &mut [Target]) {
    for i in 1..targets.len() {
        let (earlier, rest) = targets.split_at_mut(i);
        let Some(t) = rest.first_mut() else { continue };
        let mut kept: Vec<PlannedAction> = Vec::with_capacity(t.plan.actions.len());
        // Old index -> new index of the actions that stay.
        let mut new_index: Vec<Option<usize>> = Vec::with_capacity(t.plan.actions.len());
        for mut pa in t.plan.actions.drain(..) {
            // A command has no filesystem path to overlap with.
            let path = pa.action.path().map(Path::to_path_buf);
            let covered = path.as_ref().is_some_and(|path| {
                earlier
                    .iter()
                    .flat_map(|e| &e.plan.actions)
                    .any(|e| e.action.path().is_some_and(|ep| path.starts_with(ep)))
            });
            if !covered {
                // A dropped dependency was covered by an earlier plan, which
                // runs first in the batch, so there is nothing to wait for.
                pa.depends_on = pa
                    .depends_on
                    .and_then(|d| new_index.get(d).copied().flatten());
                new_index.push(Some(kept.len()));
                kept.push(pa);
                continue;
            }
            new_index.push(None);
            if let Action::EditFile { remove_lines, .. } = &pa.action {
                let first = earlier
                    .iter_mut()
                    .flat_map(|e| e.plan.actions.iter_mut())
                    .map(|e| &mut e.action)
                    .find(|a| a.path() == path.as_deref());
                if let Some(Action::EditFile {
                    remove_lines: lines,
                    ..
                }) = first
                {
                    for l in remove_lines {
                        if !lines.contains(l) {
                            lines.push(l.clone());
                        }
                    }
                }
            }
        }
        t.plan.actions = kept;
        t.plan.total_size = t.plan.actions.iter().map(|a| a.size).sum();
    }
}

/// Steps 4 to 7: preview, running-app check, confirmation, execution and
/// report. `errors` (failed sources, then notes) are warnings in the
/// preview and the envelope's `errors` under `--json`. JSON `report` has
/// one entry per plan, in order: `null` for a plan with nothing to do; the
/// whole field is `null` on a dry run.
pub fn apply(
    env: &Env,
    cfg: &Config,
    errors: &[ErrorEntry],
    out: Out,
    targets: &[Target],
    act: &ActFlags,
) -> anyhow::Result<ExitCode> {
    let plans: Vec<&Plan> = targets.iter().map(|t| &t.plan).collect();
    let empty = plans.iter().all(|p| p.actions.is_empty());

    if !act.json {
        let headed: Vec<(String, &Plan)> = targets
            .iter()
            .map(|t| (t.heading.clone(), &t.plan))
            .collect();
        render::preview(env, &headed)?;
        print_notes(errors)?;
    }
    if act.dry_run {
        if act.json {
            let data = json!({ "plans": plans, "report": null });
            print_json_entries(out.command, &data, errors)?;
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

    let interactive = prompt::is_interactive();
    for app in targets.iter().filter_map(|t| t.app) {
        let pids = running_processes(env, app);
        if pids.is_empty() {
            continue;
        }
        let list: Vec<String> = pids.iter().map(u32::to_string).collect();
        let msg = format!(
            "{} is running (pid {})",
            render::sanitise(&app.display_name),
            list.join(", ")
        );
        writeln!(anstream::stderr().lock(), "{msg}")?;
        if act.force {
            continue;
        }
        if !interactive {
            return out.fail(1, &format!("{msg}; close it first or re-run with --force"));
        }
        if !prompt::confirm(&format!(
            "{} is running; continue anyway?",
            render::sanitise(&app.display_name)
        ))? {
            return out.aborted();
        }
    }
    if !act.yes {
        if !interactive {
            return out.fail(2, NON_INTERACTIVE);
        }
        if !prompt::confirm("Proceed?")? {
            return out.aborted();
        }
    }

    let reports: Vec<Option<ExecReport>> = plans
        .iter()
        .map(|p| (!p.actions.is_empty()).then(|| execute(env, cfg, p)))
        .collect();
    let all_ok = reports
        .iter()
        .flatten()
        .all(|r| r.results.iter().all(|x| x.ok));
    if act.json {
        let data = json!({ "plans": plans, "report": reports });
        print_json_entries(out.command, &data, errors)?;
    } else {
        writeln!(anstream::stdout().lock())?;
        let runs: Vec<(&Plan, &ExecReport)> = plans
            .iter()
            .zip(&reports)
            .filter_map(|(p, r)| r.as_ref().map(|r| (*p, r)))
            .collect();
        render::report(env, &runs)?;
    }
    Ok(ExitCode::from(if all_ok { 0 } else { 3 }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crumbs_core::safety::{Fingerprint, Scope};
    use std::path::{Path, PathBuf};

    fn action(a: Action, size: u64) -> PlannedAction {
        PlannedAction {
            action: a,
            size,
            fingerprint: Fingerprint {
                dev: 0,
                ino: 0,
                is_symlink: false,
                ctime: 0,
                ctime_nsec: 0,
            },
            scope: Scope::Home,
            depends_on: None,
            reason: String::new(),
        }
    }

    fn trash(p: &str, size: u64) -> PlannedAction {
        action(Action::Trash { path: p.into() }, size)
    }

    fn edit(p: &str, line: &str) -> PlannedAction {
        action(
            Action::EditFile {
                path: p.into(),
                remove_lines: vec![line.into()],
            },
            0,
        )
    }

    fn target(actions: Vec<PlannedAction>) -> Target<'static> {
        let total_size = actions.iter().map(|a| a.size).sum();
        Target {
            heading: String::new(),
            app: None,
            plan: Plan {
                app_id: None,
                kind: None,
                actions,
                total_size,
                notes: Vec::new(),
            },
        }
    }

    fn paths(t: &Target) -> Vec<PathBuf> {
        t.plan
            .actions
            .iter()
            .filter_map(|a| a.action.path().map(Path::to_path_buf))
            .collect()
    }

    #[test]
    fn drop_overlaps_ignores_commands() {
        let cmd = || {
            action(
                Action::UserCommand {
                    program: "flatpak".into(),
                    args: Vec::new(),
                },
                0,
            )
        };
        let mut ts = [
            target(vec![cmd(), trash("/home/user/flatpak", 1)]),
            target(vec![cmd()]),
        ];
        drop_overlaps(&mut ts);
        assert_eq!(ts[0].plan.actions.len(), 2);
        assert_eq!(ts[1].plan.actions.len(), 1, "a command is never an overlap");
    }

    #[test]
    fn drop_overlaps_plans_each_path_once() {
        let mimeapps = "/home/user/.config/mimeapps.list";
        let mut ts = [
            target(vec![
                trash("/home/user/.config/shared", 10),
                edit(mimeapps, "a=a.desktop"),
            ]),
            target(vec![
                trash("/home/user/.config/shared", 10),
                trash("/home/user/.config/shared/sub", 3),
                trash("/home/user/.config/sharedish", 5),
                edit(mimeapps, "b=b.desktop"),
            ]),
        ];
        drop_overlaps(&mut ts);
        assert_eq!(
            paths(&ts[0]),
            [
                PathBuf::from("/home/user/.config/shared"),
                PathBuf::from(mimeapps)
            ]
        );
        assert_eq!(
            paths(&ts[1]),
            [PathBuf::from("/home/user/.config/sharedish")]
        );
        assert_eq!(ts[1].plan.total_size, 5);
        assert_eq!(ts[0].plan.total_size, 10);
        assert_eq!(
            ts[0].plan.actions[1].action,
            Action::EditFile {
                path: mimeapps.into(),
                remove_lines: vec!["a=a.desktop".into(), "b=b.desktop".into()],
            }
        );
    }

    #[test]
    fn drop_overlaps_remaps_depends_on() {
        let mut dependent = trash("/home/user/.config/Bar", 4);
        dependent.depends_on = Some(0);
        let mut kept_dep = trash("/home/user/.cache/Bar", 2);
        kept_dep.depends_on = Some(0);
        let mut ts = [
            target(vec![trash("/home/user/.local/share/foo", 10)]),
            target(vec![
                trash("/home/user/.local/share/foo/Bar.AppImage", 1),
                dependent,
                kept_dep,
            ]),
            // The file stays here; indexes shift when an earlier action goes.
            target(vec![
                trash("/home/user/.local/share/foo/Baz", 1),
                trash("/home/user/Apps/Q.AppImage", 1),
                {
                    let mut a = trash("/home/user/.config/Q", 1);
                    a.depends_on = Some(1);
                    a
                },
            ]),
        ];
        drop_overlaps(&mut ts);
        assert_eq!(ts[1].plan.actions.len(), 2);
        assert!(ts[1].plan.actions.iter().all(|a| a.depends_on.is_none()));
        assert_eq!(ts[2].plan.actions.len(), 2);
        assert_eq!(ts[2].plan.actions[1].depends_on, Some(0));
    }
}
