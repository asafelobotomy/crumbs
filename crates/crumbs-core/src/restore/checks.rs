//! Validating a history entry's target, trash record and backup before restoring it.

use super::*;

/// Canonical, strictly under the in-system home and not protected.
pub(super) fn valid_target(env: &Env, cfg: &Config, p: &Path) -> bool {
    is_canonical(p) && is_allowed_user_target(env, cfg, p)
}

/// Whether `root` (in-system) is a trash directory of ours on the topdir
/// above `original`: `<topdir>/.Trash-<uid>`, or the shared
/// `<topdir>/.Trash/<uid>` under a sticky, non-symlink `.Trash`. The root is
/// a real directory with mode 0700 and its `files` and `info` are real
/// directories, all owned by us; the topdir is never `/`.
pub(super) fn valid_topdir_trash(env: &Env, root: &Path, original: &Path) -> bool {
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

/// Why an entry's `trashed` record is refused.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum TrashFault {
    /// Not shaped like something `move_to_trash` produced.
    Invalid,
    /// Shaped right, but this trash root is not a private directory of
    /// ours (or a home trash outside the home).
    BadRoot(PathBuf),
}

/// The entry's `trashed` record, when it is shaped like something
/// `move_to_trash` produced for `e.path`.
#[cfg(test)]
pub(super) fn valid_trash(env: &Env, cfg: &Config, e: &HistoryEntry) -> Option<Trashed> {
    check_trash(env, cfg, e).ok()
}

pub(super) fn check_trash(
    env: &Env,
    cfg: &Config,
    e: &HistoryEntry,
) -> Result<Trashed, TrashFault> {
    let invalid = || TrashFault::Invalid;
    let t = e.trashed.as_ref().ok_or_else(invalid)?;
    if t.original != e.path || !valid_target(env, cfg, &t.original) {
        return Err(TrashFault::Invalid);
    }
    if !is_canonical(&t.files_path) || !is_canonical(&t.info_path) {
        return Err(TrashFault::Invalid);
    }
    let name = t.files_path.file_name().ok_or_else(invalid)?;
    let files_dir = t.files_path.parent().ok_or_else(invalid)?;
    if files_dir.file_name().ok_or_else(invalid)? != "files" {
        return Err(TrashFault::Invalid);
    }
    let root = files_dir.parent().ok_or_else(invalid)?;
    let home_trash = env.unsys(&env.data_home).join("Trash");
    let is_home = root == home_trash && home_trash.starts_with(env.unsys(&env.home));
    if !is_home && !valid_topdir_trash(env, root, &t.original) {
        return Err(TrashFault::BadRoot(root.to_path_buf()));
    }
    let mut info_name = name.to_os_string();
    info_name.push(".trashinfo");
    if t.info_path != root.join("info").join(info_name) {
        return Err(TrashFault::Invalid);
    }
    Ok(t.clone())
}

pub(super) fn valid_backup(env: &Env, cfg: &Config, e: &HistoryEntry, backup: &Path) -> bool {
    valid_target(env, cfg, &e.path)
        && *backup == files::with_suffix(&e.path, &format!(".crumbs-bak-{}", e.batch))
}

/// Refuses when an existing directory between the home and `in_system` is
/// a symlink, so nothing is restored through a redirected parent.
pub(super) fn check_parents(env: &Env, in_system: &Path) -> Result<(), String> {
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

pub(super) fn dry_run_trash(env: &Env, t: &Trashed) -> Result<(), String> {
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
pub(super) fn restore_backup(
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
