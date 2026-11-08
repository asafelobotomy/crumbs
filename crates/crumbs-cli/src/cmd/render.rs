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

/// Prints each plan under its heading, then the totals line.
pub fn preview(env: &Env, plans: &[(String, &Plan)], permanent: bool) -> anyhow::Result<()> {
    for (i, (heading, plan)) in plans.iter().enumerate() {
        {
            let mut out = anstream::stdout().lock();
            if i > 0 {
                writeln!(out)?;
            }
            writeln!(out, "{heading}")?;
        }
        if plan.actions.is_empty() {
            writeln!(anstream::stdout().lock(), "  nothing to remove")?;
        } else {
            let rows: Vec<Vec<String>> = plan
                .actions
                .iter()
                .map(|a| {
                    vec![
                        verb(&a.action).to_string(),
                        fmt_size(a.size),
                        display_path(env, a.action.path()),
                        a.reason.clone(),
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
            writeln!(out, "note: {n}")?;
        }
    }
    let all = || plans.iter().flat_map(|(_, p)| &p.actions);
    let removals = all()
        .filter(|a| !matches!(a.action, Action::EditFile { .. }))
        .count();
    let edits = all()
        .filter(|a| matches!(a.action, Action::EditFile { .. }))
        .count();
    let size: u64 = plans.iter().map(|(_, p)| p.total_size).sum();
    let mut out = anstream::stdout().lock();
    writeln!(out)?;
    if removals > 0 {
        if permanent {
            writeln!(
                out,
                "will permanently delete {removals} items ({})",
                fmt_size(size)
            )?;
        } else {
            writeln!(
                out,
                "will move {removals} items ({}) to the trash",
                fmt_size(size)
            )?;
        }
    }
    if edits > 0 {
        writeln!(out, "will edit {edits} files (a backup of each is kept)")?;
    }
    Ok(())
}

/// Whether a successful action leaves something `crumbs restore` can undo.
fn can_undo(a: &Action) -> bool {
    matches!(a, Action::Trash { .. } | Action::EditFile { .. })
}

/// Prints one `✓`/`✗` line per action and the closing `freed …` line.
/// Returns whether every action succeeded.
pub fn report(env: &Env, reports: &[ExecReport]) -> anyhow::Result<bool> {
    let mut out = anstream::stdout().lock();
    let mut all_ok = true;
    let mut undo: Vec<&str> = Vec::new();
    for r in reports {
        let mut restorable = false;
        for res in &r.results {
            let path = display_path(env, res.action.path());
            if res.ok {
                let done = match res.action {
                    Action::Trash { .. } => "trashed",
                    Action::Delete { .. } => "deleted",
                    Action::EditFile { .. } => "edited",
                    Action::UserCommand { .. } => "ran",
                };
                writeln!(out, "✓ {done} {path}")?;
                restorable |= can_undo(&res.action);
            } else {
                all_ok = false;
                let err = res.error.as_deref().unwrap_or("failed");
                writeln!(out, "✗ {path}: {err}")?;
            }
        }
        if restorable {
            undo.push(&r.batch);
        }
    }
    let freed: u64 = reports.iter().map(|r| r.freed).sum();
    if undo.is_empty() {
        writeln!(out, "freed {}", fmt_size(freed))?;
    } else {
        writeln!(
            out,
            "freed {} · undo with: crumbs restore {}",
            fmt_size(freed),
            undo.join(" ")
        )?;
    }
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
