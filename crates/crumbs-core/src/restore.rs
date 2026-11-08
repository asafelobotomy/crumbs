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

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RestoreOutcome {
    pub entry_id: String,
    pub path: PathBuf,
    pub ok: bool,
    pub error: Option<String>,
}

pub const NO_MATCH: &str = "no matching history entry";
pub const INVALID_ENTRY: &str = "history entry failed validation";
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
            let t = valid_trash(env, cfg, e).ok_or(INVALID_ENTRY)?;
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
    let asked = e.app_id.as_deref().and_then(|a| a.strip_prefix("pacman:"));
    let (explicit, deps): (Vec<&str>, Vec<&str>) = e
        .packages
        .iter()
        .flatten()
        .map(|(name, _)| name.as_str())
        .partition(|n| Some(*n) == asked);
    let mut commands = Vec::new();
    if !explicit.is_empty() {
        commands.push(format!("pacman -S {}", explicit.join(" ")));
    }
    if !deps.is_empty() {
        commands.push(format!("pacman -S --asdeps {}", deps.join(" ")));
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

/// Canonical, strictly under the in-system home and not protected.
fn valid_target(env: &Env, cfg: &Config, p: &Path) -> bool {
    is_canonical(p) && is_allowed_user_target(env, cfg, p)
}

/// Whether `root` (in-system) is a trash directory of ours on the topdir
/// above `original`: `<topdir>/.Trash-<uid>`, or the shared
/// `<topdir>/.Trash/<uid>` under a sticky, non-symlink `.Trash`. The root is
/// a real directory with mode 0700 and its `files` and `info` are real
/// directories, all owned by us; the topdir is never `/`.
fn valid_topdir_trash(env: &Env, root: &Path, original: &Path) -> bool {
    let uid = env.uid.to_string();
    let parent = root.parent();
    let own = format!(".Trash-{uid}");
    let (topdir, shared) = if root.file_name().is_some_and(|n| n == own.as_str()) {
        (parent, None)
    } else if root.file_name().is_some_and(|n| n == uid.as_str())
        && parent
            .and_then(Path::file_name)
            .is_some_and(|n| n == ".Trash")
    {
        (parent.and_then(Path::parent), parent)
    } else {
        return false;
    };
    let Some(topdir) = topdir else {
        return false;
    };
    if topdir == Path::new("/") || !original.starts_with(topdir) {
        return false;
    }
    let owned_dir = |p: &Path| {
        fs::symlink_metadata(env.sys_path(p))
            .ok()
            .filter(|m| m.file_type().is_dir() && m.uid() == env.uid)
    };
    if let Some(shared) = shared {
        // `symlink_metadata` does not follow, so a symlinked `.Trash` fails.
        let sticky = fs::symlink_metadata(env.sys_path(shared))
            .is_ok_and(|m| m.file_type().is_dir() && m.mode() & 0o1000 != 0);
        if !sticky {
            return false;
        }
    }
    owned_dir(root).is_some_and(|m| m.mode() & 0o777 == 0o700)
        && owned_dir(&root.join("files")).is_some()
        && owned_dir(&root.join("info")).is_some()
}

/// The entry's `trashed` record, when it is shaped like something
/// `move_to_trash` produced for `e.path`.
fn valid_trash(env: &Env, cfg: &Config, e: &HistoryEntry) -> Option<Trashed> {
    let t = e.trashed.as_ref()?;
    if t.original != e.path || !valid_target(env, cfg, &t.original) {
        return None;
    }
    if !is_canonical(&t.files_path) || !is_canonical(&t.info_path) {
        return None;
    }
    let name = t.files_path.file_name()?;
    let files_dir = t.files_path.parent()?;
    if files_dir.file_name()? != "files" {
        return None;
    }
    let root = files_dir.parent()?;
    let home_trash = env.unsys(&env.data_home).join("Trash");
    let is_home = root == home_trash && home_trash.starts_with(env.unsys(&env.home));
    if !is_home && !valid_topdir_trash(env, root, &t.original) {
        return None;
    }
    let mut info_name = name.to_os_string();
    info_name.push(".trashinfo");
    if t.info_path != root.join("info").join(info_name) {
        return None;
    }
    Some(t.clone())
}

fn valid_backup(env: &Env, cfg: &Config, e: &HistoryEntry, backup: &Path) -> bool {
    valid_target(env, cfg, &e.path)
        && *backup == files::with_suffix(&e.path, &format!(".crumbs-bak-{}", e.batch))
}

/// Refuses when an existing directory between the home and `in_system` is
/// a symlink, so nothing is restored through a redirected parent.
fn check_parents(env: &Env, in_system: &Path) -> Result<(), String> {
    let home = env.unsys(&env.home);
    let Some(parent) = in_system.parent() else {
        return Err(INVALID_ENTRY.into());
    };
    let Ok(rel) = parent.strip_prefix(&home) else {
        return Err(INVALID_ENTRY.into());
    };
    let mut dir = home.clone();
    for comp in rel.components() {
        dir.push(comp.as_os_str());
        match fs::symlink_metadata(env.sys_path(&dir)) {
            Ok(m) if m.file_type().is_symlink() => {
                return Err(format!(
                    "refusing: {} is a symlink",
                    display_path(env, &dir)
                ));
            }
            Ok(_) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => break,
            Err(e) => return Err(format!("{}: {e}", display_path(env, &dir))),
        }
    }
    Ok(())
}

fn dry_run_trash(env: &Env, t: &Trashed) -> Result<(), String> {
    if fs::symlink_metadata(env.sys_path(&t.original)).is_ok() {
        return Err(format!(
            "{}: destination already exists",
            display_path(env, &t.original)
        ));
    }
    for p in [&t.files_path, &t.info_path] {
        if fs::symlink_metadata(env.sys_path(p)).is_err() {
            return Err(format!("{}: no longer in the trash", display_path(env, p)));
        }
    }
    Ok(())
}

/// Copies the backup back over `path` via a temp file and rename. The
/// current file must be a regular file (never a symlink); its content is
/// first saved to `<path>.crumbs-bak-<batch>` so later edits survive.
/// Returns that new backup's in-system path (`None` on a dry run).
fn restore_backup(
    env: &Env,
    path: &Path,
    backup: &Path,
    dry_run: bool,
    batch: &str,
) -> Result<Option<PathBuf>, String> {
    let shown = |p: &Path, e: io::Error| format!("{}: {e}", display_path(env, p));
    let not_regular = || {
        format!(
            "refusing: {} is not a regular file",
            display_path(env, path)
        )
    };
    let real = env.sys_path(path);
    match fs::symlink_metadata(&real) {
        Ok(m) if m.file_type().is_file() => {}
        _ => return Err(not_regular()),
    }
    let saved =
        files::open_regular_nofollow(&env.sys_path(backup)).map_err(|e| shown(backup, e))?;
    let saved_mode = saved.metadata().map_err(|e| shown(backup, e))?.mode() & 0o7777;
    let bytes = files::read_all(&saved).map_err(|e| shown(backup, e))?;
    // Opened no-follow, so a swap to a symlink after the lstat is refused too.
    let current = files::open_regular_nofollow(&real).map_err(|_| not_regular())?;
    let current_mode = current.metadata().map_err(|e| shown(path, e))?.mode() & 0o7777;
    let current_bytes = files::read_all(&current).map_err(|e| shown(path, e))?;
    if dry_run {
        return Ok(None);
    }
    let new_backup = files::with_suffix(&real, &format!(".crumbs-bak-{batch}"));
    let new_backup_in_system = env.unsys(&new_backup);
    files::write_new(&new_backup, &current_bytes, current_mode)
        .map_err(|e| shown(&new_backup_in_system, e))?;
    if let Err(e) =
        files::replace_via_temp(&real, &bytes, saved_mode, &format!(".crumbs-tmp-{batch}"))
    {
        let _ = fs::remove_file(&new_backup);
        return Err(shown(path, e));
    }
    Ok(Some(new_backup_in_system))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::executor::{ExecReport, execute};
    use crate::history::{self, HistoryAction, HistoryEntry, read_history};
    use crate::plan::{Action, Plan, PlannedAction};
    use crate::safety::fingerprint;
    use crate::testing::FixtureTree;
    use std::fs;
    use std::path::Path;

    fn run(env: &Env, actions: Vec<Action>) -> ExecReport {
        let actions: Vec<PlannedAction> = actions
            .into_iter()
            .map(|action| PlannedAction {
                size: 1,
                fingerprint: fingerprint(env, action.path().unwrap()).unwrap(),
                scope: crate::safety::Scope::Home,
                depends_on: None,
                depends_on_plan: None,
                reason: "test".into(),
                action,
            })
            .collect();
        let plan = Plan {
            app_id: Some("pacman:foo".into()),
            kind: None,
            total_size: actions.len() as u64,
            actions,
            notes: Vec::new(),
        };
        let report = execute(env, &Config::default(), &plan);
        assert!(report.results.iter().all(|r| r.ok), "{report:?}");
        report
    }

    fn trash(p: &str) -> Action {
        Action::Trash { path: p.into() }
    }

    fn sel(s: &str) -> Vec<String> {
        vec![s.to_string()]
    }

    fn restore_entries(env: &Env) -> Vec<HistoryEntry> {
        read_history(env)
            .unwrap()
            .0
            .into_iter()
            .filter(|e| e.action == HistoryAction::Restore)
            .collect()
    }

    #[test]
    fn restore_by_batch() {
        let t = FixtureTree::new();
        t.write("home/user/.config/a/x", b"A");
        t.write("home/user/.cache/b/y", b"B");
        let env = t.env();
        let rep = run(
            &env,
            vec![trash("/home/user/.config/a"), trash("/home/user/.cache/b")],
        );
        let out = restore(&env, &Config::default(), &sel(&rep.batch), false).unwrap();
        assert_eq!(out.len(), 2);
        assert!(out.iter().all(|o| o.ok), "{out:?}");
        assert_eq!(out[0].entry_id, rep.results[0].history_id);
        assert_eq!(out[0].path, Path::new("/home/user/.config/a"));
        assert_eq!(fs::read(env.sys("/home/user/.config/a/x")).unwrap(), b"A");
        assert_eq!(fs::read(env.sys("/home/user/.cache/b/y")).unwrap(), b"B");
        let logged = restore_entries(&env);
        assert_eq!(logged.len(), 2);
        assert!(logged.iter().all(|e| e.ok));
        assert_eq!(
            logged[0].restores.as_deref(),
            Some(rep.results[0].history_id.as_str())
        );
        assert_ne!(logged[0].batch, rep.batch);
    }

    /// Each Restore entry is appended right after its action, on its own, so
    /// a failing append costs only that entry.
    #[test]
    fn restore_appends_per_entry() {
        use std::cell::RefCell;
        let t = FixtureTree::new();
        t.write("home/user/.config/a/x", b"A");
        t.write("home/user/.config/b/y", b"B");
        t.write("home/user/.config/c/z", b"C");
        let env = t.env();
        let rep = run(
            &env,
            vec![
                trash("/home/user/.config/a"),
                trash("/home/user/.config/b"),
                trash("/home/user/.config/c"),
            ],
        );
        let calls: RefCell<Vec<usize>> = RefCell::new(Vec::new());
        let append = |env: &Env, entries: &[HistoryEntry]| {
            let mut calls = calls.borrow_mut();
            calls.push(entries.len());
            if calls.len() == 2 {
                return Err(CoreError::Parse {
                    what: "history entry".into(),
                    message: "boom".into(),
                });
            }
            history::append(env, entries)
        };
        let out = restore_with(&env, &Config::default(), &sel(&rep.batch), false, &append).unwrap();
        assert_eq!(*calls.borrow(), [1, 1, 1]);
        assert!(out.iter().all(|o| o.ok), "{out:?}");
        assert_eq!(out[0].error, None, "{out:?}");
        assert!(
            out[1]
                .error
                .as_deref()
                .unwrap()
                .ends_with("; history not written"),
            "{out:?}"
        );
        assert_eq!(out[2].error, None, "{out:?}");
        // The restores themselves all happened, and the other two are logged.
        for d in ["a/x", "b/y", "c/z"] {
            assert!(env.sys(&format!("/home/user/.config/{d}")).exists());
        }
        let logged = restore_entries(&env);
        assert_eq!(logged.len(), 2);
        assert_eq!(
            logged[0].restores.as_deref(),
            Some(rep.results[0].history_id.as_str())
        );
        assert_eq!(
            logged[1].restores.as_deref(),
            Some(rep.results[2].history_id.as_str())
        );
    }

    #[test]
    fn restore_by_path_newest() {
        let t = FixtureTree::new();
        t.write("home/user/.config/foo/v", b"old");
        let env = t.env();
        let first = run(&env, vec![trash("/home/user/.config/foo")]);
        t.write("home/user/.config/foo/v", b"new");
        let second = run(&env, vec![trash("/home/user/.config/foo")]);
        assert_ne!(first.batch, second.batch);
        let out = restore(
            &env,
            &Config::default(),
            &sel("/home/user/.config/foo"),
            false,
        )
        .unwrap();
        assert_eq!(out.len(), 1);
        assert!(out[0].ok, "{out:?}");
        assert_eq!(out[0].entry_id, second.results[0].history_id);
        assert_eq!(
            fs::read(env.sys("/home/user/.config/foo/v")).unwrap(),
            b"new"
        );
        let (hist, _) = read_history(&env).unwrap();
        let old = hist[0].trashed.as_ref().unwrap();
        assert!(env.sys_path(&old.files_path).exists());
    }

    /// Review Focus #4.
    #[test]
    fn restore_partial_when_occupied() {
        let t = FixtureTree::new();
        t.write("home/user/.config/a/x", b"A");
        t.write("home/user/.config/b/y", b"B");
        let env = t.env();
        let rep = run(
            &env,
            vec![trash("/home/user/.config/a"), trash("/home/user/.config/b")],
        );
        t.write("home/user/.config/a/x", b"recreated");
        let out = restore(&env, &Config::default(), &sel(&rep.batch), false).unwrap();
        assert!(!out[0].ok);
        assert!(
            out[0].error.as_deref().unwrap().contains("exists"),
            "{out:?}"
        );
        assert!(out[1].ok, "{out:?}");
        assert_eq!(
            fs::read(env.sys("/home/user/.config/a/x")).unwrap(),
            b"recreated"
        );
        assert_eq!(fs::read(env.sys("/home/user/.config/b/y")).unwrap(), b"B");
        let (hist, _) = read_history(&env).unwrap();
        let a = hist[0].trashed.as_ref().unwrap();
        assert_eq!(
            fs::read(env.sys_path(&a.files_path).join("x")).unwrap(),
            b"A"
        );
        assert!(env.sys_path(&a.info_path).exists());
        let logged: Vec<bool> = restore_entries(&env).iter().map(|e| e.ok).collect();
        assert_eq!(logged, [false, true]);
    }

    #[test]
    fn restore_twice_is_noop_error() {
        let t = FixtureTree::new();
        t.write("home/user/.config/a/x", b"A");
        let env = t.env();
        let rep = run(&env, vec![trash("/home/user/.config/a")]);
        let id = rep.results[0].history_id.clone();
        assert!(restore(&env, &Config::default(), &sel(&id), false).unwrap()[0].ok);
        let again = restore(&env, &Config::default(), &sel(&id), false).unwrap();
        assert_eq!(again.len(), 1);
        assert!(!again[0].ok);
        assert_eq!(again[0].error.as_deref(), Some("already restored"));
        assert_eq!(fs::read(env.sys("/home/user/.config/a/x")).unwrap(), b"A");
    }

    fn edit_mime(env: &Env, t: &FixtureTree, original: &[u8]) -> ExecReport {
        t.write("home/user/.config/mimeapps.list", original);
        run(
            env,
            vec![Action::EditFile {
                path: "/home/user/.config/mimeapps.list".into(),
                remove_lines: vec!["text/plain=foo.desktop".into()],
            }],
        )
    }

    #[test]
    fn restore_edit_file_keeps_newer_content() {
        let t = FixtureTree::new();
        let env = t.env();
        let original = b"text/plain=foo.desktop\na=b\n";
        let rep = edit_mime(&env, &t, original);
        let file = env.sys("/home/user/.config/mimeapps.list");
        fs::write(&file, b"a=b\nnewer=de.desktop\n").unwrap();
        let out = restore(
            &env,
            &Config::default(),
            &sel(&rep.results[0].history_id),
            false,
        )
        .unwrap();
        assert!(out[0].ok, "{out:?}");
        assert_eq!(fs::read(&file).unwrap(), original);
        let logged = restore_entries(&env);
        let backup = logged[0].backup.clone().unwrap();
        assert_eq!(
            backup,
            Path::new(&format!(
                "/home/user/.config/mimeapps.list.crumbs-bak-{}",
                logged[0].batch
            ))
        );
        assert_eq!(
            fs::read(env.sys_path(&backup)).unwrap(),
            b"a=b\nnewer=de.desktop\n"
        );
    }

    #[test]
    fn restore_edit_file_refuses_symlink() {
        let t = FixtureTree::new();
        let env = t.env();
        let rep = edit_mime(&env, &t, b"text/plain=foo.desktop\na=b\n");
        let file = env.sys("/home/user/.config/mimeapps.list");
        fs::remove_file(&file).unwrap();
        t.write("home/user/other.list", b"other");
        t.symlink("home/user/.config/mimeapps.list", "../other.list");
        let out = restore(
            &env,
            &Config::default(),
            &sel(&rep.results[0].history_id),
            false,
        )
        .unwrap();
        assert!(!out[0].ok);
        assert_eq!(
            out[0].error.as_deref(),
            Some("refusing: ~/.config/mimeapps.list is not a regular file")
        );
        assert!(
            fs::symlink_metadata(&file)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(
            fs::read(env.sys("/home/user/other.list")).unwrap(),
            b"other"
        );
    }

    #[test]
    fn restore_respects_user_protect() {
        let t = FixtureTree::new();
        t.write("home/user/.config/a/x", b"A");
        let env = t.env();
        let rep = run(&env, vec![trash("/home/user/.config/a")]);
        let cfg = Config {
            protect: vec!["/home/user/.config/a".into()],
            ..Config::default()
        };
        let out = restore(&env, &cfg, &sel(&rep.batch), false).unwrap();
        assert!(!out[0].ok);
        assert_eq!(out[0].error.as_deref(), Some(INVALID_ENTRY));
        assert!(!env.sys("/home/user/.config/a").exists());
    }

    #[test]
    fn restore_edit_file_from_backup() {
        let t = FixtureTree::new();
        let original = b"[Default Applications]\ntext/plain=foo.desktop\na=b\n";
        t.write("home/user/.config/mimeapps.list", original);
        let env = t.env();
        let rep = run(
            &env,
            vec![Action::EditFile {
                path: "/home/user/.config/mimeapps.list".into(),
                remove_lines: vec!["text/plain=foo.desktop".into()],
            }],
        );
        let file = env.sys("/home/user/.config/mimeapps.list");
        assert_ne!(fs::read(&file).unwrap(), original);
        let out = restore(
            &env,
            &Config::default(),
            &sel(&rep.results[0].history_id),
            false,
        )
        .unwrap();
        assert!(out[0].ok, "{out:?}");
        assert_eq!(fs::read(&file).unwrap(), original);
    }

    #[test]
    fn restore_unknown_selector() {
        let t = FixtureTree::new();
        let env = t.env();
        let out = restore(&env, &Config::default(), &sel("12345-0"), false).unwrap();
        assert_eq!(out.len(), 1);
        assert!(!out[0].ok);
        assert_eq!(out[0].error.as_deref(), Some("no matching history entry"));
        assert!(read_history(&env).unwrap().0.is_empty());
    }

    #[test]
    fn restore_dry_run_changes_nothing() {
        let t = FixtureTree::new();
        t.write("home/user/.config/a/x", b"A");
        let env = t.env();
        let rep = run(&env, vec![trash("/home/user/.config/a")]);
        let out = restore(&env, &Config::default(), &sel(&rep.batch), true).unwrap();
        assert!(out[0].ok, "{out:?}");
        assert!(!env.sys("/home/user/.config/a").exists());
        assert!(restore_entries(&env).is_empty());
    }

    #[test]
    fn restore_rejects_tampered_entry() {
        let t = FixtureTree::new();
        t.write("home/user/.config/a/x", b"A");
        t.write("home/user/Documents/precious", b"P");
        let env = t.env();
        let rep = run(&env, vec![trash("/home/user/.config/a")]);
        let (hist, _) = read_history(&env).unwrap();
        let good = hist[0].clone();
        let mut bad = Vec::new();
        // files_path pointing at a file outside the trash.
        let mut e = good.clone();
        e.id = "1-0".into();
        e.batch = "1".into();
        if let Some(tr) = e.trashed.as_mut() {
            tr.files_path = "/home/user/Documents/precious".into();
        }
        bad.push(e);
        // Original outside home.
        let mut e = good.clone();
        e.id = "1-1".into();
        e.batch = "1".into();
        e.path = "/etc/passwd".into();
        if let Some(tr) = e.trashed.as_mut() {
            tr.original = "/etc/passwd".into();
        }
        bad.push(e);
        // Non-canonical original.
        let mut e = good.clone();
        e.id = "1-2".into();
        e.batch = "1".into();
        e.path = "/home/user/.config/../.config/a".into();
        if let Some(tr) = e.trashed.as_mut() {
            tr.original = e.path.clone();
        }
        bad.push(e);
        history::append(&env, &bad).unwrap();
        let out = restore(&env, &Config::default(), &sel("1"), false).unwrap();
        assert_eq!(out.len(), 3);
        for o in &out {
            assert!(!o.ok);
            assert_eq!(o.error.as_deref(), Some("history entry failed validation"));
        }
        assert_eq!(
            fs::read(env.sys("/home/user/Documents/precious")).unwrap(),
            b"P"
        );
        assert!(!env.sys("/home/user/.config/a").exists());
        // The genuine entry still restores.
        let out = restore(&env, &Config::default(), &sel(&rep.batch), false).unwrap();
        assert!(out[0].ok, "{out:?}");
    }

    /// A genuine trash entry rewritten to point into `root` (in-system).
    fn entry_in(env: &Env, root: &str) -> HistoryEntry {
        let mut e = read_history(env).unwrap().0[0].clone();
        let tr = e.trashed.as_mut().unwrap();
        let name = tr.files_path.file_name().unwrap().to_os_string();
        let mut info = name.clone();
        info.push(".trashinfo");
        tr.files_path = Path::new(root).join("files").join(&name);
        tr.info_path = Path::new(root).join("info").join(info);
        e
    }

    fn mode(t: &FixtureTree, rel: &str, m: u32) {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(t.dir.path().join(rel), fs::Permissions::from_mode(m)).unwrap();
    }

    fn topdir_fixture() -> (FixtureTree, Env) {
        let t = FixtureTree::new();
        t.write("home/user/.config/a/x", b"A");
        let env = t.env();
        run(&env, vec![trash("/home/user/.config/a")]);
        let uid = env.uid;
        t.mkdir(&format!("home/.Trash-{uid}/files"));
        t.mkdir(&format!("home/.Trash-{uid}/info"));
        mode(&t, &format!("home/.Trash-{uid}"), 0o700);
        (t, env)
    }

    #[test]
    fn topdir_trash_needs_private_owner_dir() {
        let (t, env) = topdir_fixture();
        let cfg = Config::default();
        let uid = env.uid;
        let root = format!("/home/.Trash-{uid}");
        let e = entry_in(&env, &root);
        assert!(valid_trash(&env, &cfg, &e).is_some());
        // Mode 0755.
        mode(&t, &format!("home/.Trash-{uid}"), 0o755);
        assert!(valid_trash(&env, &cfg, &e).is_none());
        mode(&t, &format!("home/.Trash-{uid}"), 0o700);
        assert!(valid_trash(&env, &cfg, &e).is_some());
        // Wrong owner.
        let mut other = env.clone();
        other.uid = uid.wrapping_add(1);
        let e2 = entry_in(&other, &format!("/home/.Trash-{}", other.uid));
        assert!(valid_trash(&other, &cfg, &e2).is_none());
        // `files` a symlink.
        let files = t.dir.path().join(format!("home/.Trash-{uid}/files"));
        fs::remove_dir(&files).unwrap();
        t.mkdir("home/elsewhere");
        std::os::unix::fs::symlink(t.dir.path().join("home/elsewhere"), &files).unwrap();
        assert!(valid_trash(&env, &cfg, &e).is_none());
        fs::remove_file(&files).unwrap();
        t.mkdir(&format!("home/.Trash-{uid}/files"));
        assert!(valid_trash(&env, &cfg, &e).is_some());
        // The root itself a symlink.
        let real = t.dir.path().join(format!("home/.Trash-{uid}"));
        fs::rename(&real, t.dir.path().join("home/moved")).unwrap();
        std::os::unix::fs::symlink(t.dir.path().join("home/moved"), &real).unwrap();
        assert!(valid_trash(&env, &cfg, &e).is_none());
        // A name that merely looks like a trash dir, with no directory.
        let e3 = entry_in(&env, "/home/user/.Trash-0");
        assert!(valid_trash(&env, &cfg, &e3).is_none());
    }

    #[test]
    fn shared_dot_trash_needs_sticky() {
        let (t, env) = topdir_fixture();
        let cfg = Config::default();
        let uid = env.uid;
        t.mkdir(&format!("home/.Trash/{uid}/files"));
        t.mkdir(&format!("home/.Trash/{uid}/info"));
        mode(&t, &format!("home/.Trash/{uid}"), 0o700);
        let e = entry_in(&env, &format!("/home/.Trash/{uid}"));
        // `.Trash` without the sticky bit.
        mode(&t, "home/.Trash", 0o777);
        assert!(valid_trash(&env, &cfg, &e).is_none());
        mode(&t, "home/.Trash", 0o1777);
        assert!(valid_trash(&env, &cfg, &e).is_some());
        // A symlinked `.Trash`.
        let real = t.dir.path().join("home/.Trash");
        fs::rename(&real, t.dir.path().join("home/.Trash-real")).unwrap();
        std::os::unix::fs::symlink(t.dir.path().join("home/.Trash-real"), &real).unwrap();
        assert!(valid_trash(&env, &cfg, &e).is_none());
    }

    #[test]
    fn home_trash_outside_home_not_restorable() {
        let t = FixtureTree::new();
        t.write("home/user/.config/a/x", b"A");
        let env = t.env();
        run(&env, vec![trash("/home/user/.config/a")]);
        let mut env2 = env.clone();
        env2.data_home = t.dir.path().join("data");
        let e = entry_in(&env, "/data/Trash");
        assert!(valid_trash(&env2, &Config::default(), &e).is_none());
    }

    #[test]
    fn restore_reports_commands_not_undoable() {
        let t = FixtureTree::new();
        t.write_exec("usr/bin/flatpak", b"#!/bin/sh\n");
        let env = t.env();
        let runner = crate::testing::FakeRunner::new().on(
            "flatpak",
            &["uninstall"],
            crate::command::CmdOutput {
                status: 0,
                stdout: String::new(),
                stderr: String::new(),
                timed_out: false,
            },
        );
        let plan = Plan {
            app_id: None,
            kind: None,
            actions: vec![PlannedAction {
                action: Action::UserCommand {
                    program: "flatpak".into(),
                    args: vec!["uninstall".into()],
                },
                size: 0,
                fingerprint: crate::safety::Fingerprint {
                    dev: 0,
                    ino: 0,
                    is_symlink: false,
                    ctime: 0,
                    ctime_nsec: 0,
                },
                scope: crate::safety::Scope::Home,
                depends_on: None,
                depends_on_plan: None,
                reason: "test".into(),
            }],
            total_size: 0,
            notes: Vec::new(),
        };
        let rep = crate::executor::execute_with(&env, &Config::default(), &plan, &runner);
        assert!(rep.results[0].ok);
        // By entry id: reported as not undoable.
        let id = rep.results[0].history_id.clone();
        let out = restore(&env, &Config::default(), &sel(&id), false).unwrap();
        assert_eq!(out.len(), 1);
        assert!(!out[0].ok);
        assert_eq!(out[0].error.as_deref(), Some("commands cannot be undone"));
        // By batch: commands are skipped like other no-op entries.
        let out = restore(&env, &Config::default(), &sel(&rep.batch), false).unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].error.as_deref(), Some(NO_MATCH));
    }

    #[test]
    fn restore_refuses_packages_with_reinstall_hint() {
        let t = FixtureTree::new();
        t.write_exec("usr/bin/pacman", b"#!/bin/sh\n");
        t.write_exec("usr/bin/flatpak", b"#!/bin/sh\n");
        let env = t.env();
        let unfp = |action| PlannedAction {
            action,
            size: 0,
            fingerprint: crate::safety::Fingerprint {
                dev: 0,
                ino: 0,
                is_symlink: false,
                ctime: 0,
                ctime_nsec: 0,
            },
            scope: crate::safety::Scope::Home,
            depends_on: None,
            depends_on_plan: None,
            reason: "test".into(),
        };
        let plan = Plan {
            app_id: Some("pacman:foo".into()),
            kind: None,
            actions: vec![
                unfp(Action::PackageRemove {
                    source: crate::model::SourceKind::Pacman,
                    names: vec!["foo".into(), "libfoo".into()],
                    versions: vec!["1.0-1".into(), "2.0-1".into()],
                    program: "pacman".into(),
                    args: vec!["-Rns".into(), "--noconfirm".into(), "foo".into()],
                }),
                unfp(Action::PrivilegedCommand {
                    program: "flatpak".into(),
                    args: vec!["uninstall".into()],
                }),
            ],
            total_size: 0,
            notes: Vec::new(),
        };
        let esc = crate::privilege::Escalation {
            kind: crate::privilege::EscalationKind::Pkexec,
            program: "/usr/bin/pkexec".into(),
            requires_tty: false,
        };
        let rep = crate::executor::execute_full(
            &env,
            &Config::default(),
            &plan,
            &crate::testing::FakeRunner::new(),
            &crate::testing::FakePrivilegedRunner::default(),
            Some(&esc),
            &|_, _, _| Ok(Vec::new()),
        );
        assert!(rep.results.iter().all(|r| r.ok), "{rep:?}");
        let by_id = |i: usize| {
            let id = rep.results[i].history_id.clone();
            let out = restore(&env, &Config::default(), &sel(&id), false).unwrap();
            assert_eq!(out.len(), 1);
            assert!(!out[0].ok);
            out[0].error.clone().unwrap()
        };
        assert_eq!(
            by_id(0),
            "packages cannot be restored by crumbs; reinstall with: \
             pacman -S foo && pacman -S --asdeps libfoo"
        );
        assert_eq!(by_id(1), "commands cannot be undone");
        // An entry that names no packages gives no dangling command.
        let mut e = read_history(&env).unwrap().0.remove(0);
        for packages in [None, Some(Vec::new())] {
            e.packages = packages;
            assert_eq!(
                restore_one(&env, &Config::default(), &e, true, "1"),
                Err("packages cannot be restored by crumbs".to_string())
            );
        }
        // A removal that failed part-way still says how to reinstall what
        // it removed; without an app (orphans) everything is a dependency.
        let mut e = read_history(&env).unwrap().0.remove(0);
        e.ok = false;
        e.packages = Some(vec![("libfoo".into(), "2.0-1".into())]);
        assert_eq!(
            restore_one(&env, &Config::default(), &e, true, "1"),
            Err(
                "the original removal only partly ran; packages cannot be restored by \
                 crumbs; reinstall with: pacman -S --asdeps libfoo"
                    .to_string()
            )
        );
        e.packages = Some(Vec::new());
        assert_eq!(
            restore_one(&env, &Config::default(), &e, true, "1"),
            Err("the original action failed; nothing to restore".to_string())
        );
        e.ok = true;
        e.app_id = None;
        e.packages = Some(vec![
            ("foo".into(), "1.0-1".into()),
            ("libfoo".into(), "2.0-1".into()),
        ]);
        assert_eq!(
            restore_one(&env, &Config::default(), &e, true, "1"),
            Err("packages cannot be restored by crumbs; reinstall with: \
                 pacman -S --asdeps foo libfoo"
                .to_string())
        );
        // A batch restore skips both.
        let out = restore(&env, &Config::default(), &sel(&rep.batch), false).unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].error.as_deref(), Some(NO_MATCH));
    }
}
