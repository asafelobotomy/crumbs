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

/// The `tmpfiles.d` directories read, in-system.
pub const TMPFILES_DIRS: [&str; 4] = [
    "/usr/lib/tmpfiles.d",
    "/usr/local/lib/tmpfiles.d",
    "/run/tmpfiles.d",
    "/etc/tmpfiles.d",
];

/// Paths that `tmpfiles.d` declares: runtime directories a package creates
/// without listing them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TmpfilesDeclared {
    /// Literal paths.
    paths: BTreeSet<PathBuf>,
    /// For a path with glob characters or `%` specifiers, its literal
    /// directory prefix: anything at, below, or (by the pattern) inside it
    /// may be declared.
    patterns: BTreeSet<PathBuf>,
}

impl TmpfilesDeclared {
    /// True when a declared path lies at or below `p`, or a pattern's
    /// directory lies at, below or above it (the pattern may match `p`).
    /// Components are compared whole.
    pub fn covers(&self, p: &Path) -> bool {
        let at_or_below = |set: &BTreeSet<PathBuf>| {
            set.range::<Path, _>((Bound::Included(p), Bound::Unbounded))
                .next()
                .is_some_and(|first| first.starts_with(p))
        };
        at_or_below(&self.paths)
            || at_or_below(&self.patterns)
            || self.patterns.iter().any(|d| p.starts_with(d))
    }

    /// Every literal path and pattern directory, sorted.
    pub fn all(&self) -> BTreeSet<PathBuf> {
        self.paths.union(&self.patterns).cloned().collect()
    }
}

/// The literal directory prefix of a tmpfiles path with a glob character
/// (`*?[`) or a `%` specifier: everything up to the last `/` before the
/// first such character (`/` when that is the first one). `None` for a
/// literal path.
fn pattern_dir(p: &str) -> Option<PathBuf> {
    let first = p.find(['*', '?', '[', '%'])?;
    let dir = p[..first].rfind('/').map_or("", |slash| &p[..slash]);
    Some(PathBuf::from(if dir.is_empty() { "/" } else { dir }))
}

/// Reads [`TMPFILES_DIRS`], every `*.conf`. The second field of each line
/// is the path; a path with glob characters or `%` specifiers counts by its
/// literal directory prefix ([`TmpfilesDeclared::covers`]), conservatively.
/// A path not starting with `/` (a leading specifier) is skipped. A missing
/// directory is fine; an unreadable one is an error.
pub fn tmpfiles_declared(env: &Env) -> Result<TmpfilesDeclared, CoreError> {
    let io_err = |path: &Path, source| CoreError::Io {
        path: path.to_path_buf(),
        source,
    };
    let mut out = TmpfilesDeclared::default();
    for dir in TMPFILES_DIRS {
        let dir = env.sys(dir);
        let rd = match fs::read_dir(&dir) {
            Ok(rd) => rd,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(io_err(&dir, e)),
        };
        for entry in rd {
            let path = entry.map_err(|e| io_err(&dir, e))?.path();
            if path.extension().is_none_or(|x| x != "conf") {
                continue;
            }
            let text = fs::read_to_string(&path).map_err(|e| io_err(&path, e))?;
            for line in text.lines() {
                let line = line.trim();
                if line.is_empty() || line.starts_with('#') {
                    continue;
                }
                let Some(p) = line.split_whitespace().nth(1) else {
                    continue;
                };
                if !p.starts_with('/') {
                    continue;
                }
                match pattern_dir(p) {
                    Some(d) => out.patterns.insert(d),
                    None => out.paths.insert(PathBuf::from(p.trim_end_matches('/'))),
                };
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::FixtureTree;

    #[test]
    fn tmpfiles_paths_are_parsed() {
        let t = FixtureTree::new();
        t.write(
            "usr/lib/tmpfiles.d/a.conf",
            b"# c\n\nd /etc/openvpn/client 0750 o n -\nd /run/%u 0700\nz /opt/x - - -\n",
        );
        t.write("etc/tmpfiles.d/b.conf", b"f /etc/y 0644\n");
        t.write("etc/tmpfiles.d/b.txt", b"f /etc/ignored 0644\n");
        t.write("usr/local/lib/tmpfiles.d/c.conf", b"d /opt/local-dir\n");
        t.write("run/tmpfiles.d/d.conf", b"d /etc/runtime-dir\nd %S/state\n");
        let got = tmpfiles_declared(&t.env()).unwrap();
        let want: BTreeSet<PathBuf> = [
            "/etc/openvpn/client",
            "/opt/x",
            "/etc/y",
            "/opt/local-dir",
            "/etc/runtime-dir",
            // `/run/%u`: its literal directory.
            "/run",
        ]
        .iter()
        .map(PathBuf::from)
        .collect();
        assert_eq!(got.all(), want);
        assert!(got.covers(Path::new("/opt/local-dir")));
        assert!(got.covers(Path::new("/etc/runtime-dir")));
        assert!(got.covers(Path::new("/etc/openvpn")));
        assert!(!got.covers(Path::new("/etc/ignored")));
        assert!(!got.covers(Path::new("/opt/xy")));
        assert_eq!(
            tmpfiles_declared(&FixtureTree::new().env()).unwrap(),
            TmpfilesDeclared::default()
        );
    }

    #[test]
    fn tmpfiles_globs_and_specifiers_count_by_literal_prefix() {
        assert_eq!(pattern_dir("/opt/x"), None);
        assert_eq!(pattern_dir("/etc/foo/*.json"), Some("/etc/foo".into()));
        assert_eq!(pattern_dir("/opt/foo*"), Some("/opt".into()));
        assert_eq!(pattern_dir("/opt/a/b?/c"), Some("/opt/a".into()));
        assert_eq!(pattern_dir("/opt/tpm[0-9]/x"), Some("/opt".into()));
        assert_eq!(
            pattern_dir("/var/log/journal/%m"),
            Some("/var/log/journal".into())
        );
        assert_eq!(pattern_dir("/*"), Some("/".into()));

        let t = FixtureTree::new();
        t.write(
            "usr/lib/tmpfiles.d/g.conf",
            b"d /etc/tpm2-tss/fapi-profiles/*.json\nd /opt/inky%b/run\nd /var/tmp/x-*\n",
        );
        let got = tmpfiles_declared(&t.env()).unwrap();
        // The prefix is declared: a target at or above it is covered.
        assert!(got.covers(Path::new("/etc/tpm2-tss")));
        // The pattern lives in `/opt` and may match any child of it.
        assert!(got.covers(Path::new("/opt/inky")));
        assert!(got.covers(Path::new("/opt/other")));
        assert!(!got.covers(Path::new("/etc/inky")));
        assert!(!got.covers(Path::new("/etc/tpm2")));
    }

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
