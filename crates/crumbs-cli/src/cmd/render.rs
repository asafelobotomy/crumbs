//! Human rendering of plans (the preview) and execution reports.

use std::io::Write;

use crumbs_core::Env;
use crumbs_core::env::display_path;
use crumbs_core::executor::ExecReport;
use crumbs_core::plan::{Action, Plan, PlannedAction};
use crumbs_core::safety::Scope;
use crumbs_core::size::DiskUsage;

use super::manual::{is_privileged, root_command};
use crate::output::{fmt_size, write_table_with_details};

fn verb(a: &Action) -> &'static str {
    match a {
        Action::Trash { .. } => "trash",
        Action::Delete { .. } => "delete",
        Action::EditFile { .. } => "edit",
        Action::UserCommand { .. } | Action::PrivilegedCommand { .. } => "run",
        Action::PackageRemove { .. } => "remove",
    }
}

/// The action column of a preview row: a root delete says so.
fn row_verb(a: &PlannedAction) -> &'static str {
    match (&a.action, a.scope) {
        (Action::Delete { .. }, Scope::SystemTree) => "delete (as root)",
        (action, _) => verb(action),
    }
}

/// A finding's size; `-` when part of it could not be read and nothing
/// was counted.
pub fn finding_size(size: &DiskUsage) -> String {
    if size.errors > 0 && size.bytes == 0 {
        "-".into()
    } else {
        fmt_size(size.bytes)
    }
}

/// Control characters other than tab would act on the terminal; show them
/// as U+FFFD instead. Used for text that comes from files.
pub fn sanitise(line: &str) -> String {
    line.chars()
        .map(|c| {
            if c != '\t' && c.is_control() {
                '\u{FFFD}'
            } else {
                c
            }
        })
        .collect()
}

/// A path as shown to people: home as `~`, control characters replaced.
/// JSON output keeps the raw path.
pub fn shown_path(env: &Env, in_system: &std::path::Path) -> String {
    sanitise(&display_path(env, in_system))
}

/// What a row shows for the action's target: the path, or for a command
/// its whole command line.
fn target(env: &Env, a: &Action) -> String {
    match a {
        Action::UserCommand { program, args } | Action::PrivilegedCommand { program, args } => {
            let mut line = program.clone();
            for arg in args {
                line.push(' ');
                line.push_str(arg);
            }
            sanitise(&line)
        }
        Action::PackageRemove { source, names, .. } => sanitise(&format!(
            "{}: {}",
            source.as_str(),
            names.first().map_or("", String::as_str)
        )),
        _ => a.path().map(|p| shown_path(env, p)).unwrap_or_default(),
    }
}

/// [`target`] as the preview shows it: a package removal also names the
/// version of the app's own package.
fn preview_target(env: &Env, a: &Action) -> String {
    match a {
        Action::PackageRemove {
            source,
            names,
            versions,
            ..
        } => sanitise(&format!(
            "{}: {} {}",
            source.as_str(),
            names.first().map_or("", String::as_str),
            versions.first().map_or("", String::as_str)
        )),
        _ => target(env, a),
    }
}

/// `also removed: <name> <version>, …` for the packages a removal takes
/// besides the app's own, or `None`.
fn also_removed(a: &Action) -> Option<String> {
    let Action::PackageRemove {
        names, versions, ..
    } = a
    else {
        return None;
    };
    // Every name, even one without a version: nothing may go unlisted.
    let others: Vec<String> = names
        .iter()
        .enumerate()
        .skip(1)
        .map(|(i, n)| match versions.get(i) {
            Some(v) => format!("{n} {v}"),
            None => n.clone(),
        })
        .collect();
    (!others.is_empty()).then(|| sanitise(&format!("also removed: {}", others.join(", "))))
}

pub const NOTE_PRIVILEGED: &str =
    "these steps run with administrator rights; you will be asked once";

/// The size column of a preview row: `—` for a command (the program
/// decides what goes), `-` for an action whose size could not be read.
fn row_size(a: &PlannedAction) -> String {
    match a.action {
        Action::UserCommand { .. }
        | Action::PrivilegedCommand { .. }
        | Action::PackageRemove { .. } => "—".to_string(),
        _ if !a.size_known => "-".to_string(),
        _ => fmt_size(a.size),
    }
}

/// A totals line's size: `X + unknown` when some action's size is unknown.
fn total(size: u64, unknown: bool) -> String {
    if unknown {
        format!("{} + unknown", fmt_size(size))
    } else {
        fmt_size(size)
    }
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// Prints each plan under its heading, then the totals lines, to stdout
/// with colour (anstream drops it where the terminal does not want it).
pub fn preview(env: &Env, plans: &[(String, &Plan)]) -> anyhow::Result<()> {
    let mut text = String::new();
    preview_to(&mut text, env, plans, true)?;
    anstream::stdout().lock().write_all(text.as_bytes())?;
    Ok(())
}

/// Writes each plan under its heading, then the totals lines: trashed and
/// deleted items are counted apart, whatever `--permanent` was. The table
/// headers are bold only with `color`.
pub fn preview_to(
    w: &mut dyn std::fmt::Write,
    env: &Env,
    plans: &[(String, &Plan)],
    color: bool,
) -> std::fmt::Result {
    for (i, (heading, plan)) in plans.iter().enumerate() {
        if i > 0 {
            writeln!(w)?;
        }
        writeln!(w, "{}", sanitise(heading))?;
        if plan.actions.is_empty() {
            writeln!(w, "  nothing to remove")?;
        } else {
            let rows: Vec<Vec<String>> = plan
                .actions
                .iter()
                .map(|a| {
                    vec![
                        row_verb(a).to_string(),
                        row_size(a),
                        preview_target(env, &a.action),
                        sanitise(&a.reason),
                    ]
                })
                .collect();
            // An edit shows the exact lines it removes; a privileged
            // action the exact command run as root.
            let details: Vec<Vec<String>> = plan
                .actions
                .iter()
                .map(|a| match &a.action {
                    Action::EditFile { remove_lines, .. } => {
                        remove_lines.iter().map(|l| sanitise(l)).collect()
                    }
                    _ => root_command(env, a)
                        .map(|c| vec![format!("as root: {c}")])
                        .unwrap_or_default(),
                })
                .collect();
            write_table_with_details(
                w,
                &["ACTION", "SIZE", "PATH", "REASON"],
                &rows,
                &details,
                color,
            )?;
        }
        for line in plan.actions.iter().filter_map(|a| also_removed(&a.action)) {
            writeln!(w, "{line}")?;
        }
        for n in &plan.notes {
            writeln!(w, "note: {}", sanitise(n))?;
        }
    }
    let all = || plans.iter().flat_map(|(_, p)| &p.actions);
    if all().any(is_privileged) {
        writeln!(w, "note: {NOTE_PRIVILEGED}")?;
    }
    // (count, size, whether some size is unknown)
    let tally = |want: fn(&Action) -> bool| {
        all()
            .filter(|a| want(&a.action))
            .fold((0usize, 0u64, false), |(n, size, unknown), a| {
                (n + 1, size + a.size, unknown || !a.size_known)
            })
    };
    let (trashed, trash_size, trash_unknown) = tally(|a| matches!(a, Action::Trash { .. }));
    let (deleted, delete_size, delete_unknown) = tally(|a| matches!(a, Action::Delete { .. }));
    let (edits, ..) = tally(|a| matches!(a, Action::EditFile { .. }));
    let (commands, ..) = tally(|a| {
        matches!(
            a,
            Action::UserCommand { .. } | Action::PrivilegedCommand { .. }
        )
    });
    writeln!(w)?;
    if trashed > 0 {
        writeln!(
            w,
            "will move {} ({}) to the trash",
            plural(trashed, "item", "items"),
            total(trash_size, trash_unknown)
        )?;
    }
    if deleted > 0 {
        writeln!(
            w,
            "will permanently delete {} ({})",
            plural(deleted, "item", "items"),
            total(delete_size, delete_unknown)
        )?;
    }
    if edits > 0 {
        writeln!(w, "will edit {edits} files (a backup of each is kept)")?;
    }
    if commands > 0 {
        writeln!(w, "will run {}", plural(commands, "command", "commands"))?;
    }
    Ok(())
}

/// Whether a successful action leaves something `crumbs restore` can undo.
fn can_undo(a: &Action) -> bool {
    matches!(a, Action::Trash { .. } | Action::EditFile { .. })
}

/// Prints one `✓`/`✗` line per action and the closing line: `moved <size>
/// to the trash` for what was trashed, `freed <size>` for what was deleted,
/// a mention of a command that ran (its size is unknown), and the undo hint
/// when anything can be restored. With none of these it says `freed 0 B`. Each report goes with the plan it ran. Returns whether
/// every action succeeded.
pub fn report(env: &Env, runs: &[(&Plan, &ExecReport)]) -> anyhow::Result<bool> {
    report_to(&mut anstream::stdout().lock(), env, runs)
}

/// [`report`] to any writer.
pub(crate) fn report_to(
    out: &mut dyn Write,
    env: &Env,
    runs: &[(&Plan, &ExecReport)],
) -> anyhow::Result<bool> {
    let mut all_ok = true;
    let mut undo: Vec<&str> = Vec::new();
    let (mut moved, mut freed) = (0u64, 0u64);
    let (mut any_moved, mut any_freed) = (false, false);
    // A command frees space it decides itself; unless it was measured,
    // its size is unknown.
    let mut ran: Option<&str> = None;
    for (plan, r) in runs {
        let mut restorable = false;
        for (res, planned) in r.results.iter().zip(&plan.actions) {
            let shown = target(env, &res.action);
            if res.ok {
                let done = match res.action {
                    Action::Trash { .. } => {
                        any_moved = true;
                        moved = moved.saturating_add(planned.size);
                        "trashed"
                    }
                    Action::Delete { .. } => {
                        any_freed = true;
                        freed = freed.saturating_add(planned.size);
                        "deleted"
                    }
                    Action::EditFile { .. } => "edited",
                    Action::UserCommand { ref program, .. }
                    | Action::PrivilegedCommand { ref program, .. } => {
                        match res.freed {
                            Some(bytes) => {
                                any_freed = true;
                                freed = freed.saturating_add(bytes);
                            }
                            None => {
                                ran.get_or_insert(program);
                            }
                        }
                        "ran"
                    }
                    Action::PackageRemove { .. } => "removed",
                };
                match res.freed {
                    Some(bytes) => writeln!(out, "✓ {done} {shown} (freed {})", fmt_size(bytes))?,
                    None => writeln!(out, "✓ {done} {shown}")?,
                }
                restorable |= can_undo(&res.action);
            } else {
                all_ok = false;
                let err = sanitise(res.error.as_deref().unwrap_or("failed"));
                writeln!(out, "✗ {shown}: {err}")?;
            }
        }
        if restorable {
            undo.push(&r.batch);
        }
    }
    let mut parts: Vec<String> = Vec::new();
    if any_moved {
        parts.push(format!("moved {} to the trash", fmt_size(moved)));
    }
    if any_freed {
        parts.push(format!("freed {}", fmt_size(freed)));
    }
    match ran.map(sanitise) {
        Some(program) if parts.is_empty() => {
            parts.push(format!("freed space decided by {program}"));
        }
        Some(program) => parts.push(format!("plus space freed by {program}")),
        None if parts.is_empty() => parts.push(format!("freed {}", fmt_size(freed))),
        None => {}
    }
    if !undo.is_empty() {
        parts.push(format!("undo with: crumbs restore {}", undo.join(" ")));
    }
    writeln!(out, "{}", parts.join(" · "))?;
    Ok(all_ok)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn also_removed_lists_every_name() {
        let a = Action::PackageRemove {
            source: crumbs_core::SourceKind::Pacman,
            names: vec!["app".into(), "b".into(), "c".into()],
            versions: vec!["1".into(), "2".into()],
            program: "pacman".into(),
            args: Vec::new(),
        };
        assert_eq!(also_removed(&a).as_deref(), Some("also removed: b 2, c"));
    }

    #[test]
    fn unknown_size_shows_dash_and_unknown_total() {
        use crumbs_core::safety::Fingerprint;
        let mut a = PlannedAction {
            action: Action::Delete {
                path: "/opt/inky".into(),
            },
            size: 0,
            fingerprint: Fingerprint::default(),
            scope: Scope::SystemTree,
            depends_on: None,
            depends_on_plan: None,
            size_known: false,
            reason: String::new(),
        };
        assert_eq!(row_size(&a), "-");
        assert_eq!(total(4096, true), "4.0 KiB + unknown");
        assert_eq!(total(4096, false), "4.0 KiB");
        // JSON is unchanged: the size is 0 and nothing says it is unknown.
        let v = serde_json::to_value(&a).unwrap();
        assert_eq!(v["size"], 0);
        assert!(v.get("size_known").is_none(), "{v}");
        a.size_known = true;
        assert_eq!(row_size(&a), fmt_size(0));
    }

    /// The output of [`report_to`] for one plan of command actions, each
    /// with the measured space its result carries.
    fn report_text(cmds: &[(&str, &[&str], Option<u64>)]) -> String {
        use crumbs_core::executor::{ActionResult, ExecReport};
        use crumbs_core::safety::Fingerprint;
        let actions: Vec<PlannedAction> = cmds
            .iter()
            .map(|(program, args, _)| PlannedAction {
                action: Action::PrivilegedCommand {
                    program: (*program).into(),
                    args: args.iter().map(|a| (*a).to_string()).collect(),
                },
                size: 0,
                fingerprint: Fingerprint::default(),
                scope: Scope::System,
                depends_on: None,
                depends_on_plan: None,
                size_known: true,
                reason: String::new(),
            })
            .collect();
        let results = actions
            .iter()
            .zip(cmds)
            .map(|(pa, (_, _, freed))| ActionResult {
                action: pa.action.clone(),
                ok: true,
                error: None,
                history_id: "1-0".into(),
                rights_missing: false,
                freed: *freed,
            })
            .collect();
        let plan = Plan {
            app_id: None,
            kind: Some("clean".into()),
            total_size: 0,
            actions,
            notes: Vec::new(),
        };
        let report = ExecReport {
            batch: "1".into(),
            results,
            freed: 0,
            escalation_failed: false,
            batch_stopped: false,
            escalation_output: Vec::new(),
        };
        let t = crumbs_core::testing::FixtureTree::new();
        let mut out = Vec::new();
        report_to(&mut out, &t.env(), &[(&plan, &report)]).unwrap();
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn measured_command_counts_as_freed() {
        let text = report_text(&[("paccache", &["-rk2"], Some(5 * 1024 * 1024))]);
        assert!(
            text.contains("✓ ran paccache -rk2 (freed 5.0 MiB)"),
            "{text}"
        );
        assert!(text.trim_end().ends_with("freed 5.0 MiB"), "{text}");
        assert!(!text.contains("plus space"), "{text}");
    }

    #[test]
    fn unmeasured_command_still_says_so() {
        let text = report_text(&[
            ("paccache", &["-rk2"], Some(1024 * 1024)),
            ("journalctl", &["--vacuum-size=500M"], None),
        ]);
        assert!(
            text.contains("✓ ran journalctl --vacuum-size=500M\n"),
            "{text}"
        );
        assert!(
            text.trim_end()
                .ends_with("freed 1.0 MiB · plus space freed by journalctl"),
            "{text}"
        );
    }

    #[test]
    fn only_trash_and_edit_can_be_undone() {
        let p = std::path::PathBuf::from("/x");
        assert!(can_undo(&Action::Trash { path: p.clone() }));
        assert!(can_undo(&Action::EditFile {
            path: p.clone(),
            remove_lines: Vec::new()
        }));
        assert!(!can_undo(&Action::Delete { path: p }));
        assert!(!can_undo(&Action::UserCommand {
            program: "flatpak".into(),
            args: Vec::new()
        }));
    }
}
