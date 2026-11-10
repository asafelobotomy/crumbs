//! Deleting and editing files, and the basenames too generic to match a process by.

use super::*;

/// Removes the entry itself: a symlink or file is unlinked, a real
/// directory is removed recursively (std's `remove_dir_all` does not follow
/// symlinks inside the tree). A directory holding a mount point (another
/// filesystem, or a bind mount of the same one) is refused untouched.
pub(super) fn delete(env: &Env, path: &Path) -> Result<(), String> {
    delete_with(env, path, &size::mount_id)
}

/// [`delete`] with the mount lookup injected (tests fake a mount point).
pub(super) fn delete_with(
    env: &Env,
    path: &Path,
    mount_of: &dyn Fn(&Path, &fs::Metadata) -> Option<u64>,
) -> Result<(), String> {
    let real = env.sys_path(path);
    let meta = fs::symlink_metadata(&real).map_err(|e| io_msg(env, path, &e))?;
    if !meta.file_type().is_dir() {
        return fs::remove_file(&real).map_err(|e| io_msg(env, path, &e));
    }
    // `remove_dir_all` would empty a filesystem mounted inside the tree.
    match size::crosses_mount(&real, mount_of) {
        Ok(false) => {}
        Ok(true) => {
            return Err(format!(
                "refusing: {} contains another filesystem",
                display_path(env, path)
            ));
        }
        Err(e) => {
            return Err(format!(
                "refusing: cannot check {} for other filesystems: {e}",
                display_path(env, path)
            ));
        }
    }
    let mut removed = fs::remove_dir_all(&real);
    if removed
        .as_ref()
        .is_err_and(|e| e.kind() == std::io::ErrorKind::PermissionDenied)
    {
        // Read-only folders (Go's module cache): make the ones you own
        // writable again, as `go clean -modcache` does, and try once more.
        make_owned_dirs_writable(&real, env.uid);
        removed = fs::remove_dir_all(&real);
    }
    removed.map_err(|e| {
        let msg = io_msg(env, path, &e);
        // Part of the tree may already be gone; say so.
        if fs::symlink_metadata(&real).is_ok() {
            format!("partially deleted: {msg}")
        } else {
            msg
        }
    })
}

/// Gives every real directory in the tree at `real` that `uid` owns its
/// owner's read, write and search bits, so what is in it can be removed.
/// Nothing is made writable for anyone else.
///
/// Walked by file descriptor, never by name: each directory is opened
/// `O_PATH|O_NOFOLLOW`, checked with `fstat`, changed through its
/// `/proc/self/fd` link (the inode that was opened) and read through a
/// descriptor of that same inode, so a name swapped for a symlink
/// mid-walk leads nowhere.
pub(super) fn make_owned_dirs_writable(real: &Path, uid: u32) {
    use rustix::fs::{CWD, Dir, FileType, Mode, OFlags, chmod, fstat, open, openat};
    use std::os::fd::{AsRawFd, OwnedFd};

    fn walk(dir: OwnedFd, uid: u32) {
        let Ok(st) = fstat(&dir) else { return };
        if FileType::from_raw_mode(st.st_mode) != FileType::Directory || st.st_uid != uid {
            return;
        }
        let magic = format!("/proc/self/fd/{}", dir.as_raw_fd());
        let mode = st.st_mode & 0o7777;
        if mode & 0o700 != 0o700 {
            let _ = chmod(magic.as_str(), Mode::from_raw_mode(mode | 0o700));
        }
        let Ok(readable) = open(
            magic.as_str(),
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
            Mode::empty(),
        ) else {
            return;
        };
        let Ok(entries) = Dir::read_from(&readable) else {
            return;
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            if name.to_bytes() == b"." || name.to_bytes() == b".." {
                continue;
            }
            if let Ok(child) = openat(
                &readable,
                name,
                OFlags::PATH | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::empty(),
            ) {
                walk(child, uid);
            }
        }
    }

    if let Ok(root) = openat(
        CWD,
        real,
        OFlags::PATH | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    ) {
        walk(root, uid);
    }
}

/// Filters `path` in place. Returns the in-system backup path, or `None`
/// when no line matched (nothing is written then).
pub(super) fn edit_file(
    env: &Env,
    path: &Path,
    remove_lines: &[String],
    fp: &Fingerprint,
    batch: &str,
) -> Result<Option<PathBuf>, String> {
    let real = env.sys_path(path);
    let err = |e: io::Error| io_msg(env, path, &e);
    if fs::symlink_metadata(&real)
        .map_err(err)?
        .file_type()
        .is_symlink()
    {
        return Err(format!(
            "refusing: {} is a symlink; edit it by hand",
            display_path(env, path)
        ));
    }
    // Still opened no-follow in case it is swapped after the check.
    let file = files::open_regular_nofollow(&real).map_err(err)?;
    let meta = file.metadata().map_err(err)?;
    if meta.dev() != fp.dev || meta.ino() != fp.ino {
        return Err(format!(
            "refusing: {} changed since it was scanned",
            display_path(env, path)
        ));
    }
    let original = files::read_all(&file).map_err(err)?;
    let filtered = files::filter_lines(&original, remove_lines);
    if filtered == original {
        return Ok(None);
    }
    let mode = meta.mode() & 0o7777;
    let backup = files::with_suffix(&real, &format!(".crumbs-bak-{batch}"));
    let backup_in_system = env.unsys(&backup);
    files::write_new(&backup, &original, mode).map_err(|e| io_msg(env, &backup_in_system, &e))?;
    if let Err(e) = files::replace_via_temp(&real, &filtered, mode, &format!(".crumbs-tmp-{batch}"))
    {
        let _ = fs::remove_file(&backup);
        return Err(err(e));
    }
    Ok(Some(backup_in_system))
}

/// Names too common to say which app a process belongs to: never matched
/// by basename (exe, comm or argv0).
pub(super) const GENERIC_BASENAMES: [&str; 15] = [
    "sh", "bash", "dash", "zsh", "fish", "env", "python", "python3", "perl", "ruby", "node",
    "electron", "java", "flatpak", "bwrap",
];

/// Whether `n` is a [`GENERIC_BASENAMES`] name or `python3.<digits>`.
pub(super) fn is_generic(n: &OsStr) -> bool {
    GENERIC_BASENAMES.iter().any(|g| OsStr::new(g) == n)
        || n.as_bytes()
            .strip_prefix(b"python3.")
            .is_some_and(|v| !v.is_empty() && v.iter().all(u8::is_ascii_digit))
}
