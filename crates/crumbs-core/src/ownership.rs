//! Index of package-owned files and directories.

use std::collections::BTreeSet;
use std::fs;
use std::ops::Bound;
use std::path::{Path, PathBuf};

use crate::env::Env;
use crate::error::CoreError;
use crate::sources::pacman::{DB_DIR, owned_entries};

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
mod tests {
    use super::*;
    use crate::testing::FixtureTree;

    #[test]
    fn dirs_from_files_section_are_owned() {
        let t = FixtureTree::new();
        t.pacman_pkg("foo", "1-1", Some(1), &["opt/foo/", "opt/foo/bin/foo"]);
        let idx = load_pacman(&t.env()).unwrap().unwrap();
        assert!(idx.owns(Path::new("/opt/foo")));
        assert!(idx.owns_at_or_below(Path::new("/opt/foo")));
        assert!(idx.owns(Path::new("/opt/foo/bin/foo")));
        assert!(!idx.is_empty());
    }

    #[test]
    fn prefix_is_component_wise() {
        let t = FixtureTree::new();
        t.pacman_pkg("foobar", "1-1", Some(1), &["opt/foobar/x"]);
        let idx = load_pacman(&t.env()).unwrap().unwrap();
        assert!(!idx.owns_at_or_below(Path::new("/opt/foo")));
        assert!(idx.owns_at_or_below(Path::new("/opt/foobar")));
    }

    #[test]
    fn unowned_tree_is_unowned() {
        let t = FixtureTree::new();
        t.pacman_pkg("foo", "1-1", Some(1), &["opt/foo/"]);
        let idx = load_pacman(&t.env()).unwrap().unwrap();
        assert!(!idx.owns(Path::new("/opt/inky")));
        assert!(!idx.owns_at_or_below(Path::new("/opt/inky")));
    }

    #[test]
    fn no_pacman_db_is_none() {
        let t = FixtureTree::new();
        assert_eq!(load_pacman(&t.env()).unwrap(), None);
    }

    #[test]
    fn non_utf8_owned_dir_is_owned() {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;
        let t = FixtureTree::new();
        t.write(
            "/var/lib/pacman/local/caf-1-1/files",
            b"%FILES%\nopt/caf\xe9/\nopt/caf\xe9/x\n",
        );
        let idx = load_pacman(&t.env()).unwrap().unwrap();
        let dir = Path::new(OsStr::from_bytes(b"/opt/caf\xe9"));
        assert!(idx.owns(dir));
        assert!(idx.owns_dir(dir));
        assert!(idx.owns_at_or_below(dir));
        assert!(!idx.owns_at_or_below(Path::new("/opt/caf\u{fffd}")));
    }

    #[test]
    fn unreadable_files_entry_is_an_error() {
        let t = FixtureTree::new();
        t.pacman_pkg("ok", "1-1", Some(1), &["opt/ok/"]);
        // `files` is a directory, so reading it fails.
        t.write("/var/lib/pacman/local/bad-1-1/files/x", b"");
        assert!(load_pacman(&t.env()).is_err());
    }

    #[test]
    fn sibling_ordering_does_not_hide_children() {
        let t = FixtureTree::new();
        t.pacman_pkg(
            "sib",
            "1-1",
            Some(1),
            &["opt/foo-x/", "opt/foo.d/", "opt/foo/bar"],
        );
        let idx = load_pacman(&t.env()).unwrap().unwrap();
        assert!(idx.owns_at_or_below(Path::new("/opt/foo")));
        assert!(!idx.owns(Path::new("/opt/foo")));

        let t = FixtureTree::new();
        t.pacman_pkg("sib", "1-1", Some(1), &["opt/foo-x/", "opt/foo.d/"]);
        let idx = load_pacman(&t.env()).unwrap().unwrap();
        assert!(!idx.owns_at_or_below(Path::new("/opt/foo")));
    }

    #[test]
    fn trailing_and_duplicate_slashes_normalise() {
        let t = FixtureTree::new();
        t.pacman_pkg("sl", "1-1", Some(1), &["opt//foo//", "opt//foo//bin/x"]);
        let idx = load_pacman(&t.env()).unwrap().unwrap();
        assert!(idx.owns_dir(Path::new("/opt/foo")));
        assert!(idx.owns(Path::new("/opt/foo/bin/x")));
        assert!(idx.is_complete());
    }

    #[test]
    fn bad_queries_are_unowned() {
        let t = FixtureTree::new();
        t.pacman_pkg("p", "1-1", Some(1), &["opt/foo/"]);
        let idx = load_pacman(&t.env()).unwrap().unwrap();
        for q in ["", "opt/foo", "foo"] {
            assert!(!idx.owns(Path::new(q)), "{q}");
            assert!(!idx.owns_at_or_below(Path::new(q)), "{q}");
        }
    }
}
