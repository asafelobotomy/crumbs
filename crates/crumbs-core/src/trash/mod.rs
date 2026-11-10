//! Freedesktop trash (spec 1.0), implemented through `Env` so tests can use a
//! fake root. Only `$topdir/.Trash-$uid` is ever created, never `.Trash/$uid`.

use std::ffi::OsStr;
use std::fs::{self, DirBuilder, OpenOptions};
use std::io::{self, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::env::Env;
use crate::error::CoreError;
use crate::safety::is_canonical;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Trashed {
    pub original: PathBuf,
    pub files_path: PathBuf,
    pub info_path: PathBuf,
}

fn io_err(path: &Path, source: io::Error) -> CoreError {
    CoreError::Io {
        path: path.to_path_buf(),
        source,
    }
}

fn mkdir_private(path: &Path) -> Result<(), CoreError> {
    DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(path)
        .map_err(|e| io_err(path, e))
}

/// The trash dir must be a real directory (not a symlink) owned by us with no
/// group/other access.
pub(crate) fn validate_private_dir(env: &Env, path: &Path) -> Result<(), CoreError> {
    let m = fs::symlink_metadata(path).map_err(|e| io_err(path, e))?;
    let bad = |msg: &str| {
        io_err(
            path,
            io::Error::new(io::ErrorKind::PermissionDenied, msg.to_string()),
        )
    };
    if !m.file_type().is_dir() {
        return Err(bad("trash path is not a real directory"));
    }
    if m.uid() != env.uid {
        return Err(bad("trash directory is not owned by the current user"));
    }
    if m.mode() & 0o022 != 0 {
        return Err(bad("trash directory is writable by other users"));
    }
    Ok(())
}

/// Creates (if needed) and validates a trash directory.
fn ensure_private_dir(env: &Env, path: &Path) -> Result<(), CoreError> {
    mkdir_private(path)?;
    validate_private_dir(env, path)
}

/// Rename that never replaces an existing destination (RENAME_NOREPLACE),
/// falling back to plain rename only where the filesystem lacks support.
fn rename_noreplace(src: &Path, dst: &Path) -> io::Result<()> {
    use rustix::fs::{CWD, RenameFlags, renameat_with};
    use rustix::io::Errno;
    match renameat_with(CWD, src, CWD, dst, RenameFlags::NOREPLACE) {
        Ok(()) => Ok(()),
        Err(Errno::INVAL | Errno::NOSYS) => fs::rename(src, dst),
        Err(e) => Err(io::Error::from(e)),
    }
}

fn dev_of_real(p: &Path) -> Option<u64> {
    fs::symlink_metadata(p).ok().map(|m| m.dev())
}

/// Highest ancestor of `path` that is still on `path`'s device. `None` when
/// the device of `path` is unknown or no ancestor shares it.
pub(crate) fn topdir_for(
    path: &Path,
    root: &Path,
    dev_of: impl Fn(&Path) -> Option<u64>,
) -> Option<PathBuf> {
    // The walk never leaves the root: a fake root must not see the host's
    // directories above it.
    if !path.starts_with(root) {
        return None;
    }
    let dev = dev_of(path)?;
    let mut top: Option<&Path> = None;
    for a in path.ancestors().skip(1) {
        if dev_of(a) == Some(dev) {
            top = Some(a);
        } else {
            break;
        }
        if a == root {
            break;
        }
    }
    top.map(Path::to_path_buf)
}

/// Nearest existing ancestor (or self) of `p`, for device lookup.
fn nearest_existing(p: &Path) -> Option<&Path> {
    p.ancestors().find(|a| fs::symlink_metadata(a).is_ok())
}

/// The trash directory (real path) for an item at `in_system`, created with
/// mode 0700 when missing.
pub fn trash_dir_for(env: &Env, in_system: &Path) -> Result<PathBuf, CoreError> {
    let item = env.sys_path(in_system);
    let home_trash = env.data_home.join("Trash");
    // Restore and the safety checks only accept a home trash under home.
    let home_trash_dir = |env: &Env| {
        if !env.unsys(&home_trash).starts_with(env.unsys(&env.home)) {
            return Err(io_err(
                in_system,
                io::Error::other("the trash folder is outside your home directory"),
            ));
        }
        ensure_private_dir(env, &home_trash)?;
        Ok(home_trash.clone())
    };
    // A missing item cannot be moved anyway; let the rename report that.
    let Some(item_dev) = dev_of_real(&item) else {
        return home_trash_dir(env);
    };
    let data_dev = nearest_existing(&env.data_home).and_then(dev_of_real);
    if data_dev == Some(item_dev) {
        return home_trash_dir(env);
    }
    let fail = |msg: &str| io_err(in_system, io::Error::other(msg.to_string()));
    let top =
        topdir_for(&item, &env.root, dev_of_real).ok_or_else(|| fail("no usable trash topdir"))?;
    if env.unsys(&top) == Path::new("/") {
        return Err(fail("refusing to use / as a trash topdir"));
    }
    let dir = top.join(format!(".Trash-{}", env.uid));
    ensure_topdir_trash(env, &dir)?;
    Ok(dir)
}

/// Creates (if missing) a topdir trash root and applies the rule restore
/// applies to it (`restore.rs::valid_topdir_trash`): a real directory owned
/// by us with mode exactly 0700. Crumbs never trashes into a root it would
/// refuse to restore from.
fn ensure_topdir_trash(env: &Env, dir: &Path) -> Result<(), CoreError> {
    ensure_private_dir(env, dir)?;
    let m = fs::symlink_metadata(dir).map_err(|e| io_err(dir, e))?;
    if m.mode() & 0o777 != 0o700 {
        return Err(io_err(
            dir,
            io::Error::new(
                io::ErrorKind::PermissionDenied,
                "trash directory must be a private 0700 directory",
            ),
        ));
    }
    Ok(())
}

/// Percent-encodes an absolute path; keeps `A-Za-z0-9-._~/`.
fn encode_path(p: &Path) -> String {
    let mut out = String::new();
    for &b in p.as_os_str().as_bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~' | b'/') {
            out.push(char::from(b));
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn info_name(info: &Path, name: &OsStr) -> PathBuf {
    let mut f = name.to_os_string();
    f.push(".trashinfo");
    info.join(f)
}

fn name_taken(files: &Path, info: &Path, name: &OsStr) -> bool {
    fs::symlink_metadata(files.join(name)).is_ok()
        || fs::symlink_metadata(info_name(info, name)).is_ok()
}

/// Moves the item itself (never a symlink target) into the trash.
pub fn move_to_trash(env: &Env, in_system: &Path) -> Result<Trashed, CoreError> {
    if !is_canonical(in_system) {
        return Err(io_err(
            in_system,
            io::Error::new(io::ErrorKind::InvalidInput, "path is not canonical"),
        ));
    }
    let item = env.sys_path(in_system);
    let trash = trash_dir_for(env, in_system)?;
    let files = trash.join("files");
    let info = trash.join("info");
    ensure_private_dir(env, &files)?;
    ensure_private_dir(env, &info)?;

    let base = in_system
        .file_name()
        .ok_or_else(|| {
            io_err(
                in_system,
                io::Error::new(io::ErrorKind::InvalidInput, "path has no file name"),
            )
        })?
        .to_os_string();
    let content = format!(
        "[Trash Info]\nPath={}\nDeletionDate={}\n",
        encode_path(in_system),
        jiff::Zoned::now().strftime("%Y-%m-%dT%H:%M:%S")
    );

    let mut n = 1u32;
    let (name, info_path) = loop {
        let name = if n == 1 {
            base.clone()
        } else {
            let mut s = base.clone();
            s.push(format!(".{n}"));
            s
        };
        n += 1;
        if name_taken(&files, &info, &name) {
            continue;
        }
        let info_path = info_name(&info, &name);
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&info_path)
        {
            Ok(mut f) => {
                f.write_all(content.as_bytes())
                    .map_err(|e| io_err(&info_path, e))
                    .inspect_err(|_| {
                        let _ = fs::remove_file(&info_path);
                    })?;
                break (name, info_path);
            }
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(io_err(&info_path, e)),
        }
    };

    let files_path = files.join(&name);
    if let Err(e) = rename_noreplace(&item, &files_path) {
        let _ = fs::remove_file(&info_path);
        return Err(io_err(in_system, e));
    }
    Ok(Trashed {
        original: in_system.to_path_buf(),
        files_path: env.unsys(&files_path),
        info_path: env.unsys(&info_path),
    })
}

/// Moves a trashed item back. Refuses when the original path is occupied.
pub fn restore_from_trash(env: &Env, t: &Trashed) -> Result<(), CoreError> {
    let info = env.sys_path(&t.info_path);
    let recorded = fs::read_to_string(&info).map_err(|e| io_err(&t.info_path, e))?;
    let expected = encode_path(&t.original);
    let matches = recorded
        .lines()
        .any(|l| l.strip_prefix("Path=") == Some(expected.as_str()));
    if !matches {
        return Err(io_err(
            &t.info_path,
            io::Error::new(io::ErrorKind::InvalidData, "trash entry no longer matches"),
        ));
    }
    let dest = env.sys_path(&t.original);
    if fs::symlink_metadata(&dest).is_ok() {
        return Err(io_err(
            &t.original,
            io::Error::new(io::ErrorKind::AlreadyExists, "destination already exists"),
        ));
    }
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent).map_err(|e| io_err(&t.original, e))?;
    }
    let src = env.sys_path(&t.files_path);
    rename_noreplace(&src, &dest).map_err(|e| io_err(&t.files_path, e))?;
    fs::remove_file(&info).map_err(|e| io_err(&t.info_path, e))
}

#[cfg(test)]
mod tests;
