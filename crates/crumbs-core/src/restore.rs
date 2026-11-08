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
use crate::safety::{is_allowed_user_target, is_canonical};
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

/// Restores the entries the selectors name. A selector is an entry id, a
/// batch id (its successful entries that changed something) or an absolute
/// in-system path (the newest successful `Trash` entry for it). Unless
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
        let matched = select(&entries, sel);
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

/// Entries a selector names, oldest first. `Restore` entries are never
/// selected.
fn select<'a>(entries: &'a [HistoryEntry], sel: &str) -> Vec<&'a HistoryEntry> {
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
        .collect();
    if !batch.is_empty() {
        return batch;
    }
    if sel.starts_with('/') {
        let path = Path::new(sel);
        if let Some(e) =
            actions().rfind(|e| e.action == HistoryAction::Trash && e.ok && e.path == path)
        {
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
    if !e.ok {
        return Err("the original action failed; nothing to restore".into());
    }
    match e.action {
        HistoryAction::Delete => Err("permanently deleted; cannot be restored".into()),
        HistoryAction::Restore => Err(INVALID_ENTRY.into()),
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

/// Canonical, strictly under the in-system home and not protected.
fn valid_target(env: &Env, cfg: &Config, p: &Path) -> bool {
    is_canonical(p) && is_allowed_user_target(env, cfg, p)
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
    let topdir_trash = format!(".Trash-{}", env.uid);
    if root != home_trash && root.file_name()? != topdir_trash.as_str() {
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
                fingerprint: fingerprint(env, action.path()).unwrap(),
                scope: crate::safety::Scope::Home,
                depends_on: None,
                reason: "test".into(),
                action,
            })
            .collect();
        let plan = Plan {
            app_id: Some("pacman:foo".into()),
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
}
