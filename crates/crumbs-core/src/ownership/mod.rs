//! Index of package-owned files and directories.

use std::collections::BTreeSet;
use std::fs;
use std::ops::Bound;
use std::path::{Path, PathBuf};

use crate::env::Env;
use crate::error::CoreError;
use crate::sources::pacman::{DB_DIR, owned_entries};

mod tmpfiles;
pub use tmpfiles::*;
/// Package-owned files and directories.
///
/// `complete` is true only when every package entry was read; callers that
/// treat "not owned" as permission to act must check it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct OwnedIndex {
    /// Files and directories together, ordered for range queries.
    all: BTreeSet<PathBuf>,
    dirs: BTreeSet<PathBuf>,
    complete: bool,
}

impl OwnedIndex {
    pub fn new(
        files: impl IntoIterator<Item = PathBuf>,
        dirs: impl IntoIterator<Item = PathBuf>,
        complete: bool,
    ) -> Self {
        let dirs: BTreeSet<PathBuf> = dirs.into_iter().collect();
        let mut all: BTreeSet<PathBuf> = files.into_iter().collect();
        all.extend(dirs.iter().cloned());
        Self {
            all,
            dirs,
            complete,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.all.is_empty()
    }

    pub fn is_complete(&self) -> bool {
        self.complete
    }

    /// True when a package owns exactly this file or directory. Empty and
    /// relative paths are never owned.
    pub fn owns(&self, p: &Path) -> bool {
        p.is_absolute() && self.all.contains(p)
    }

    /// True when a package owns this directory.
    pub fn owns_dir(&self, p: &Path) -> bool {
        p.is_absolute() && self.dirs.contains(p)
    }

    /// True when a package owns `p` or anything beneath it. Components are
    /// compared whole, so `/opt/foo` does not match `/opt/foobar`. Empty and
    /// relative paths give false.
    pub fn owns_at_or_below(&self, p: &Path) -> bool {
        p.is_absolute()
            && self
                .all
                .range::<Path, _>((Bound::Included(p), Bound::Unbounded))
                .next()
                .is_some_and(|first| first.starts_with(p))
    }
}

/// The package database ownership is read from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackageDb {
    Pacman,
    Dpkg,
    Rpm,
}

/// A dpkg database with content: a non-empty status file and an info
/// directory. An empty one (dpkg installed as a tool on another distro) is
/// not a package database.
pub fn dpkg_present(env: &Env) -> bool {
    let status = env.sys(crate::sources::dpkg::STATUS);
    fs::metadata(&status).is_ok_and(|m| m.is_file() && m.len() > 0)
        && env.sys(crate::sources::dpkg::INFO_DIR).is_dir()
}

/// An rpm database: the `rpm` tool and a non-empty rpmdb directory
/// (`/usr/lib/sysimage/rpm` on current Fedora, `/var/lib/rpm` before).
pub fn rpm_present(env: &Env) -> bool {
    let has_entries = |p: &str| fs::read_dir(env.sys(p)).is_ok_and(|mut rd| rd.next().is_some());
    crate::command::resolve_trusted(env, "rpm").is_some()
        && (has_entries("/usr/lib/sysimage/rpm") || has_entries("/var/lib/rpm"))
}

/// The first package database present, in the order pacman, dpkg, rpm.
pub fn detect_package_db(env: &Env) -> Option<PackageDb> {
    if env.sys(DB_DIR).is_dir() {
        Some(PackageDb::Pacman)
    } else if dpkg_present(env) {
        Some(PackageDb::Dpkg)
    } else if rpm_present(env) {
        Some(PackageDb::Rpm)
    } else {
        None
    }
}

/// The owned index from every package database present (pacman, dpkg and
/// rpm together, as the inventory combines them); `None` when there is
/// none. Any present database that cannot be read completely is an error:
/// see [`load_pacman`].
pub fn load_package_db(
    env: &Env,
    runner: &dyn crate::command::CommandRunner,
) -> Result<Option<OwnedIndex>, CoreError> {
    let parts = [load_pacman(env)?, load_dpkg(env)?, load_rpm(env, runner)?];
    let mut found = false;
    let mut all = Vec::new();
    let mut dirs = Vec::new();
    for idx in parts.into_iter().flatten() {
        found = true;
        all.extend(idx.all);
        dirs.extend(idx.dirs);
    }
    Ok(found.then(|| OwnedIndex::new(all, dirs, true)))
}

/// Every path the dpkg database lists, as a complete index; `None` when
/// there is no dpkg database. Any unreadable list is an error.
pub fn load_dpkg(env: &Env) -> Result<Option<OwnedIndex>, CoreError> {
    if !dpkg_present(env) {
        return Ok(None);
    }
    let files = crate::sources::dpkg::owned_paths(env)?;
    Ok(Some(OwnedIndex::new(files, Vec::<PathBuf>::new(), true)))
}

/// Every path the rpm database lists, from one `rpm -qa` query; `None`
/// when `rpm` is not installed. A failed or undecodable query is an error.
pub fn load_rpm(
    env: &Env,
    runner: &dyn crate::command::CommandRunner,
) -> Result<Option<OwnedIndex>, CoreError> {
    if !rpm_present(env) {
        return Ok(None);
    }
    let files = crate::sources::rpm::query(env, runner)?
        .into_iter()
        .flat_map(|p| p.files);
    Ok(Some(OwnedIndex::new(files, Vec::<PathBuf>::new(), true)))
}

/// Reads every package's file list from the pacman database. `None` when the
/// database is missing. Any unreadable entry is an error: a partial index
/// would report owned paths as unowned.
pub fn load_pacman(env: &Env) -> Result<Option<OwnedIndex>, CoreError> {
    let db = env.sys(DB_DIR);
    if !db.is_dir() {
        return Ok(None);
    }
    let io_err = |path: &Path, source| CoreError::Io {
        path: path.to_path_buf(),
        source,
    };
    let mut files = Vec::new();
    let mut dirs = Vec::new();
    for entry in fs::read_dir(&db).map_err(|e| io_err(&db, e))? {
        let dir = entry.map_err(|e| io_err(&db, e))?.path();
        if !dir.is_dir() {
            continue;
        }
        let list = dir.join("files");
        let bytes = fs::read(&list).map_err(|e| io_err(&list, e))?;
        let (f, d) = owned_entries(&bytes);
        files.extend(f);
        dirs.extend(d);
    }
    Ok(Some(OwnedIndex::new(files, dirs, true)))
}

#[cfg(test)]
mod tests;
