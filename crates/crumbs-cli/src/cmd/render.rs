//! Human rendering of plans (the preview) and execution reports.

use std::io::Write;

use crumbs_core::Env;
use crumbs_core::env::display_path;
use crumbs_core::executor::ExecReport;
use crumbs_core::plan::{Action, Plan};

use crate::output::{fmt_size, print_table_with_details};

fn verb(a: &Action) -> &'static str {
    match a {
        Action::Trash { .. } => "trash",
        Action::Delete { .. } => "delete",
        Action::EditFile { .. } => "edit",
        Action::UserCommand { .. } => "run",
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
        Action::UserCommand { program, args } => {
            let mut line = program.clone();
            for arg in args {
                line.push(' ');
                line.push_str(arg);
            }
            sanitise(&line)
        }
        _ => shown_path(env, a.path()),
    }
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// Prints each plan under its heading, then the totals lines: trashed and
/// deleted items are counted apart, whatever `--permanent` was.
pub fn preview(env: &Env, plans: &[(String, &Plan)]) -> anyhow::Result<()> {
    for (i, (heading, plan)) in plans.iter().enumerate() {
        {
            let mut out = anstream::stdout().lock();
            if i > 0 {
                writeln!(out)?;
            }
            writeln!(out, "{}", sanitise(heading))?;
        }
        if plan.actions.is_empty() {
            writeln!(anstream::stdout().lock(), "  nothing to remove")?;
        } else {
            let rows: Vec<Vec<String>> = plan
                .actions
                .iter()
                .map(|a| {
                    let size = match a.action {
                        // Flatpak decides what goes; the size is unknown.
                        Action::UserCommand { .. } => "—".to_string(),
                        _ => fmt_size(a.size),
                    };
                    vec![
                        verb(&a.action).to_string(),
                        size,
                        target(env, &a.action),
                        sanitise(&a.reason),
                    ]
                })
                .collect();
            // An edit shows the exact lines it removes.
            let details: Vec<Vec<String>> = plan
                .actions
                .iter()
                .map(|a| match &a.action {
                    Action::EditFile { remove_lines, .. } => {
                        remove_lines.iter().map(|l| sanitise(l)).collect()
                    }
                    _ => Vec::new(),
                })
                .collect();
            print_table_with_details(&["ACTION", "SIZE", "PATH", "REASON"], &rows, &details)?;
        }
        let mut out = anstream::stdout().lock();
        for n in &plan.notes {
            writeln!(out, "note: {}", sanitise(n))?;
        }
    }
    let all = || plans.iter().flat_map(|(_, p)| &p.actions);
    let tally = |want: fn(&Action) -> bool| {
        all()
            .filter(|a| want(&a.action))
            .fold((0usize, 0u64), |(n, size), a| (n + 1, size + a.size))
    };
    let (trashed, trash_size) = tally(|a| matches!(a, Action::Trash { .. }));
    let (deleted, delete_size) = tally(|a| matches!(a, Action::Delete { .. }));
    let (edits, _) = tally(|a| matches!(a, Action::EditFile { .. }));
    let (commands, _) = tally(|a| matches!(a, Action::UserCommand { .. }));
    let mut out = anstream::stdout().lock();
    writeln!(out)?;
    if trashed > 0 {
        writeln!(
            out,
            "will move {} ({}) to the trash",
            plural(trashed, "item", "items"),
            fmt_size(trash_size)
        )?;
    }
    if deleted > 0 {
        writeln!(
            out,
            "will permanently delete {} ({})",
            plural(deleted, "item", "items"),
            fmt_size(delete_size)
        )?;
    }
    if edits > 0 {
        writeln!(out, "will edit {edits} files (a backup of each is kept)")?;
    }
    if commands > 0 {
        writeln!(out, "will run {}", plural(commands, "command", "commands"))?;
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
    let mut out = anstream::stdout().lock();
    let mut all_ok = true;
    let mut undo: Vec<&str> = Vec::new();
    let (mut moved, mut freed) = (0u64, 0u64);
    let (mut any_moved, mut any_freed) = (false, false);
    // A command frees space it decides itself; its size is unknown.
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
                    Action::UserCommand { ref program, .. } => {
                        ran.get_or_insert(program);
                        "ran"
                    }
                };
                writeln!(out, "✓ {done} {shown}")?;
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
