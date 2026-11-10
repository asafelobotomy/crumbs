//! Undo: puts trashed items and edited files back, driven by the history.
//!
//! History lines are untrusted input: every entry is validated (canonical
//! paths under the home, trash paths shaped like a real trash entry, the
//! backup named exactly as the executor names it) before anything is moved.

use std::collections::HashSet;
use std::fs;
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::config::Config;
use crate::env::{Env, display_path};
use crate::error::CoreError;
use crate::executor::files;
use crate::history::{self, HistoryAction, HistoryEntry};
use crate::safety::{Scope, is_allowed_user_target, is_canonical};
use crate::trash::{self, Trashed};

mod checks;
use checks::*;
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RestoreOutcome {
    pub entry_id: String,
    pub path: PathBuf,
    pub ok: bool,
    pub error: Option<String>,
}

pub const NO_MATCH: &str = "no matching history entry";
pub const INVALID_ENTRY: &str = "history entry failed validation";
/// A recognised batch with nothing to put back: not an error, just no work.
pub const NOTHING_RESTORABLE: &str = "nothing in this batch can be restored";
pub const ALREADY_RESTORED: &str = "already restored";
/// Why a `Delete` of a [`Scope::SystemTree`](crate::safety::Scope::SystemTree)
/// target cannot be undone.
pub const MSG_SYSTEM_TREE: &str = "system files deleted as root cannot be restored";

/// Restores the entries the selectors name. A selector is an entry id, a
/// batch id (its successful entries that changed something) or an absolute
/// in-system path (the newest successful `Trash` entry for it that has not
/// been restored yet). Unless
/// `dry_run`, every attempt on a matched entry is logged as a `Restore`
/// entry whose `restores` is the restored entry's id.
pub fn restore(
    env: &Env,
    cfg: &Config,
    selectors: &[String],
    dry_run: bool,
) -> Result<Vec<RestoreOutcome>, CoreError> {
    restore_with(env, cfg, selectors, dry_run, &history::append)
}

type Append<'a> = dyn Fn(&Env, &[HistoryEntry]) -> Result<(), CoreError> + 'a;

/// [`restore`] with the history writer injected (tests fail one entry).
fn restore_with(
    env: &Env,
    cfg: &Config,
    selectors: &[String],
    dry_run: bool,
    append: &Append<'_>,
) -> Result<Vec<RestoreOutcome>, CoreError> {
    let (entries, _) = history::read_history(env)?;
    let restored: HashSet<&str> = entries
        .iter()
        .filter(|e| e.action == HistoryAction::Restore && e.ok)
        .filter_map(|e| e.restores.as_deref())
        .collect();
    let batch = history::next_batch_id(env);
    let mut seen: HashSet<&str> = HashSet::new();
    let mut outcomes = Vec::new();
    let mut logged = 0usize;
    for sel in selectors {
        let matched = select(&entries, sel, &restored);
        if matched.is_empty() && is_batch(&entries, sel) {
            outcomes.push(RestoreOutcome {
                entry_id: String::new(),
                path: PathBuf::from(sel),
                ok: true,
                error: Some(NOTHING_RESTORABLE.into()),
            });
            continue;
        }
        if matched.is_empty() {
            outcomes.push(RestoreOutcome {
                entry_id: String::new(),
                path: PathBuf::from(sel),
                ok: false,
                error: Some(NO_MATCH.into()),
            });
            continue;
        }
        for e in matched {
            if !seen.insert(e.id.as_str()) {
                continue;
            }
            let res = if restored.contains(e.id.as_str()) {
                Err(ALREADY_RESTORED.to_string())
            } else {
                restore_one(env, cfg, e, dry_run, &batch)
            };
            let (backup, mut error) = match res {
                Ok(b) => (b, None),
                Err(e) => (None, Some(e)),
            };
            // What happened on disk; a history failure doesn't change it.
            let ok = error.is_none();
            if !dry_run {
                let entry = HistoryEntry {
                    id: format!("{batch}-{logged}"),
                    batch: batch.clone(),
                    time: history::now_rfc3339(),
                    action: HistoryAction::Restore,
                    path: e.path.clone(),
                    trashed: e.trashed.clone(),
                    backup,
                    size: e.size,
                    ok,
                    error: error.clone(),
                    app_id: e.app_id.clone(),
                    restores: Some(e.id.clone()),
                    argv: None,
                    packages: None,
                    source: None,
                    kind: None,
                    scope: None,
                };
                logged += 1;
                // Appended right after its action, so a later failure never
                // costs this record and a failure here costs only this one.
                if let Err(err) = append(env, std::slice::from_ref(&entry)) {
                    let suffix = format!("{err}; history not written");
                    error = Some(match error {
                        Some(prev) => format!("{prev}; {suffix}"),
                        None => suffix,
                    });
                }
            }
            outcomes.push(RestoreOutcome {
                entry_id: e.id.clone(),
                path: e.path.clone(),
                ok,
                error,
            });
        }
    }
    Ok(outcomes)
}

/// Whether `sel` is the batch id of a recorded action.
fn is_batch(entries: &[HistoryEntry], sel: &str) -> bool {
    entries
        .iter()
        .any(|e| e.action != HistoryAction::Restore && e.batch == sel)
}

/// Entries a selector names, oldest first. A path names its newest entry
/// not yet restored. `Restore` entries are never selected; a batch skips
/// `Command`, `PrivilegedCommand`, `PackageRemove` and `Delete` entries.
fn select<'a>(
    entries: &'a [HistoryEntry],
    sel: &str,
    restored: &HashSet<&str>,
) -> Vec<&'a HistoryEntry> {
    let actions = || {
        entries
            .iter()
            .filter(|e| e.action != HistoryAction::Restore)
    };
    if let Some(e) = actions().find(|e| e.id == sel) {
        return vec![e];
    }
    let batch: Vec<&HistoryEntry> = actions()
        .filter(|e| e.batch == sel && e.ok)
        .filter(|e| !(e.action == HistoryAction::EditFile && e.backup.is_none()))
        // A command, a package removal or a permanent delete left nothing
        // to put back; named by its id, such an entry still says why.
        .filter(|e| {
            !matches!(
                e.action,
                HistoryAction::Command
                    | HistoryAction::Delete
                    | HistoryAction::PackageRemove
                    | HistoryAction::PrivilegedCommand
            )
        })
        .collect();
    if !batch.is_empty() {
        return batch;
    }
    if sel.starts_with('/') {
        let path = Path::new(sel);
        // The newest entry that is still in the trash.
        if let Some(e) = actions().rfind(|e| {
            e.action == HistoryAction::Trash
                && e.ok
                && e.path == path
                && !restored.contains(e.id.as_str())
        }) {
            return vec![e];
        }
    }
    Vec::new()
}

/// On success, the in-system path of the backup taken of the current file
/// (EditFile restores only).
fn restore_one(
    env: &Env,
    cfg: &Config,
    e: &HistoryEntry,
    dry_run: bool,
    batch: &str,
) -> Result<Option<PathBuf>, String> {
    if e.action == HistoryAction::PackageRemove {
        return Err(reinstall_hint(e));
    }
    // Said whether or not the delete ran: nothing deleted as root comes back.
    if e.action == HistoryAction::Delete && e.scope == Some(Scope::SystemTree) {
        return Err(MSG_SYSTEM_TREE.into());
    }
    if !e.ok {
        return Err("the original action failed; nothing to restore".into());
    }
    match e.action {
        HistoryAction::Delete if e.scope == Some(Scope::SystemTree) => Err(MSG_SYSTEM_TREE.into()),
        HistoryAction::Delete => Err("permanently deleted; cannot be restored".into()),
        HistoryAction::Restore => Err(INVALID_ENTRY.into()),
        HistoryAction::Command | HistoryAction::PrivilegedCommand => {
            Err("commands cannot be undone".into())
        }
        HistoryAction::PackageRemove => Err(reinstall_hint(e)),
        HistoryAction::Trash => {
            let t = check_trash(env, cfg, e).map_err(|f| match f {
                TrashFault::Invalid => INVALID_ENTRY.to_string(),
                TrashFault::BadRoot(root) => format!(
                    "trash directory {} has unsafe permissions or owner \
                     (needs a private 0700 directory owned by you)",
                    display_path(env, &root)
                ),
            })?;
            check_parents(env, &t.original)?;
            if dry_run {
                return dry_run_trash(env, &t).map(|()| None);
            }
            trash::restore_from_trash(env, &t)
                .map(|()| None)
                .map_err(|e| e.to_string())
        }
        HistoryAction::EditFile => {
            let Some(backup) = &e.backup else {
                return Err("nothing was changed; no backup to restore".into());
            };
            if !valid_backup(env, cfg, e, backup) {
                return Err(INVALID_ENTRY.into());
            }
            check_parents(env, &e.path)?;
            restore_backup(env, &e.path, backup, dry_run, batch)
        }
    }
}

/// Why a package removal cannot be restored, with the pacman commands that
/// reinstall what it removed (`packages`), also when the removal failed
/// part-way. The package the entry's `app_id` names (`pacman:<name>`, the
/// one the user asked to uninstall) is reinstalled explicitly; the others
/// were removed as its dependencies, or as orphans, so `--asdeps` keeps
/// them marked as dependencies.
fn reinstall_hint(e: &HistoryEntry) -> String {
    const CANNOT: &str = "packages cannot be restored by crumbs";
    let source = e.source.unwrap_or(crate::model::SourceKind::Pacman);
    let prefix_id = format!("{}:", source.as_str());
    let asked = e
        .app_id
        .as_deref()
        .and_then(|a| a.strip_prefix(prefix_id.as_str()));
    let (explicit, deps): (Vec<&str>, Vec<&str>) = e
        .packages
        .iter()
        .flatten()
        .map(|(name, _)| name.as_str())
        .partition(|n| Some(*n) == asked);
    let mut commands = Vec::new();
    match source {
        crate::model::SourceKind::Dpkg => {
            if !explicit.is_empty() {
                commands.push(format!("apt-get install {}", explicit.join(" ")));
            }
            if !deps.is_empty() {
                let d = deps.join(" ");
                commands.push(format!("apt-get install {d} && apt-mark auto {d}"));
            }
        }
        crate::model::SourceKind::Rpm | crate::model::SourceKind::Snap => {
            let tool = if source == crate::model::SourceKind::Rpm {
                "dnf"
            } else {
                "snap"
            };
            let all: Vec<&str> = explicit.iter().chain(deps.iter()).copied().collect();
            if !all.is_empty() {
                commands.push(format!("{tool} install {}", all.join(" ")));
            }
        }
        _ => {
            if !explicit.is_empty() {
                commands.push(format!("pacman -S {}", explicit.join(" ")));
            }
            if !deps.is_empty() {
                commands.push(format!("pacman -S --asdeps {}", deps.join(" ")));
            }
        }
    }
    let prefix = if e.ok {
        CANNOT.to_string()
    } else {
        format!("the original removal only partly ran; {CANNOT}")
    };
    if commands.is_empty() {
        if e.ok {
            prefix
        } else {
            "the original action failed; nothing to restore".into()
        }
    } else {
        format!("{prefix}; reinstall with: {}", commands.join(" && "))
    }
}

#[cfg(test)]
mod tests;
