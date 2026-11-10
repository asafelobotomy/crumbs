//! Plans for `crumbs clean`: bloat items, with their notes.

use super::*;

pub const NOTE_SYSTEM_DELETE: &str =
    "system files are deleted permanently with administrator rights and cannot be restored";
pub const NOTE_TRASH_FREES_LATER: &str =
    "space is freed when the trash is emptied (or use --permanent)";
/// The note for a plan that trashes: on a disk over 90% full, that the
/// trash frees nothing now (the trash is on the home filesystem);
/// otherwise [`NOTE_TRASH_FREES_LATER`].
pub(super) fn trash_note(space: Option<(u64, u64)>) -> String {
    match space {
        Some((avail, total)) if total > 0 && avail.saturating_mul(10) < total => {
            let used = total - avail;
            // Rounded up; reserved blocks count as used, so this can read a
            // point above df's figure.
            let pct = (u128::from(used) * 100).div_ceil(u128::from(total));
            format!(
                "the disk is {pct}% full ({} free): moving to the trash frees nothing until \
                 the trash is emptied; use --permanent to free the space now",
                crate::size::fmt_size(avail)
            )
        }
        _ => NOTE_TRASH_FREES_LATER.into(),
    }
}

pub const NOTE_PERMANENT_TEMP: &str = "permanently delete (temporary files)";
pub const NOTE_PERMANENT_TRASH: &str = "permanently delete (emptying the trash)";

/// Builds a plan for `clean` from scanned items. Trash items are trashed
/// (deleted with `--permanent`), delete items are always deleted, command
/// items become `UserCommand` and privileged items `PrivilegedCommand`. A
/// `UserTemp` or `HomeTrash` item may only be deleted, never trashed. Extra
/// paths (a trashed entry's `.trashinfo`) are deleted right after their item
/// and depend on it.
pub fn build_clean_plan(
    env: &Env,
    items: &[BloatItem],
    opts: &PlanOptions,
) -> Result<Plan, CoreError> {
    let mut actions: Vec<PlannedAction> = Vec::new();
    for item in items {
        let reason = item.note.clone().unwrap_or_else(|| item.label.clone());
        let bad = |message: &str| CoreError::Parse {
            what: format!("clean item {}", item.label),
            message: message.into(),
        };
        let command = match &item.action {
            ItemAction::Command { program, args } => Some(Action::UserCommand {
                program: program.clone(),
                args: args.clone(),
            }),
            // Run as root in the run's one batch; the executor resolves the
            // bare program name from the trusted directories.
            ItemAction::Privileged { program, args } => Some(Action::PrivilegedCommand {
                program: program.clone(),
                args: args.clone(),
            }),
            ItemAction::Trash | ItemAction::Delete => None,
        };
        if let Some(action) = command {
            actions.push(PlannedAction {
                action,
                size: item.size.unwrap_or(0),
                fingerprint: Fingerprint::default(),
                scope: item.scope,
                depends_on: None,
                depends_on_plan: None,
                size_known: true,
                reason,
            });
            continue;
        }
        let path = item.path.clone().ok_or_else(|| bad("item has no path"))?;
        let fingerprint = item
            .fingerprint
            .ok_or_else(|| bad("item has no fingerprint"))?;
        let action = match item.action {
            ItemAction::Trash if item.scope != Scope::Home => {
                return Err(bad("items outside the home can only be deleted"));
            }
            ItemAction::Trash if !opts.permanent => Action::Trash { path },
            _ => Action::Delete { path },
        };
        let main = actions.len();
        actions.push(PlannedAction {
            action,
            size: item.size.unwrap_or(0),
            fingerprint,
            scope: item.scope,
            depends_on: None,
            depends_on_plan: None,
            size_known: true,
            reason: reason.clone(),
        });
        for (extra, fp) in &item.extra_paths {
            actions.push(PlannedAction {
                action: Action::Delete {
                    path: extra.clone(),
                },
                size: 0,
                fingerprint: *fp,
                scope: item.scope,
                depends_on: Some(main),
                depends_on_plan: None,
                size_known: true,
                reason: reason.clone(),
            });
        }
    }
    let mut notes = Vec::new();
    if actions
        .iter()
        .any(|a| matches!(a.action, Action::Trash { .. }))
    {
        // The trash is on the home filesystem.
        notes.push(trash_note(env.fs_space(&env.unsys(&env.home))));
    }
    if actions.iter().any(|a| a.scope == Scope::UserTemp) {
        notes.push(NOTE_PERMANENT_TEMP.into());
    }
    if actions.iter().any(|a| a.scope == Scope::HomeTrash) {
        notes.push(NOTE_PERMANENT_TRASH.into());
    }
    let total_size = actions.iter().map(|a| a.size).sum();
    Ok(Plan {
        app_id: None,
        kind: Some(crate::history::KIND_CLEAN.into()),
        actions,
        total_size,
        notes,
    })
}
