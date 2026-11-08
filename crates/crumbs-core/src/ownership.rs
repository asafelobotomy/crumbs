//! Index of package-owned files and directories.

use std::collections::{BTreeSet, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use crate::env::Env;
use crate::error::CoreError;
use crate::sources::pacman::{DB_DIR, owned_entries};

#[derive(Debug, Clone, Default, PartialEq)]
pub struct OwnedIndex {
    pub files: HashSet<PathBuf>,
    pub dirs: BTreeSet<PathBuf>,
    /// Files and directories together, ordered for range queries.
    all: BTreeSet<PathBuf>,
}

impl OwnedIndex {
    pub fn new(files: HashSet<PathBuf>, dirs: BTreeSet<PathBuf>) -> Self {
        let all = files.iter().chain(dirs.iter()).cloned().collect();
        Self { files, dirs, all }
    }

    pub fn is_empty(&self) -> bool {
        self.all.is_empty()
    }

    /// True when a package owns exactly this file or directory.
    pub fn owns(&self, p: &Path) -> bool {
        self.files.contains(p) || self.dirs.contains(p)
    }

    /// True when a package owns `p` or anything beneath it. Components are
    /// compared whole, so `/opt/foo` does not match `/opt/foobar`.
    pub fn owns_at_or_below(&self, p: &Path) -> bool {
        self.all
            .range(p.to_path_buf()..)
            .next()
            .is_some_and(|first| first.starts_with(p))
    }
}

/// Reads every package's file list from the pacman database. `None` when the
/// database is missing.
pub fn load_pacman(env: &Env) -> Result<Option<OwnedIndex>, CoreError> {
    let db = env.sys(DB_DIR);
    if !db.is_dir() {
        return Ok(None);
    }
    let io_err = |path: &Path, source| CoreError::Io {
        path: path.to_path_buf(),
        source,
    };
    let mut files = HashSet::new();
    let mut dirs = BTreeSet::new();
    for entry in fs::read_dir(&db).map_err(|e| io_err(&db, e))? {
        let dir = entry.map_err(|e| io_err(&db, e))?.path();
        if !dir.is_dir() {
            continue;
        }
        let list = dir.join("files");
        let bytes = fs::read(&list).map_err(|e| io_err(&list, e))?;
        let (f, d) = owned_entries(&String::from_utf8_lossy(&bytes));
        files.extend(f.into_iter().map(PathBuf::from));
        dirs.extend(d.into_iter().map(PathBuf::from));
    }
    Ok(Some(OwnedIndex::new(files, dirs)))
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
}
