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

/// The package database ownership is read from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackageDb {
    Pacman,
    Dpkg,
    Rpm,
}

/// The first package database present, in the order pacman, dpkg, rpm.
pub fn detect_package_db(env: &Env) -> Option<PackageDb> {
    if env.sys(DB_DIR).is_dir() {
        Some(PackageDb::Pacman)
    } else if env.sys("/var/lib/dpkg/status").is_file() {
        Some(PackageDb::Dpkg)
    } else if crate::command::resolve_trusted(env, "rpm").is_some() {
        Some(PackageDb::Rpm)
    } else {
        None
    }
}

/// The owned index from whichever package database is present; `None`
/// when there is none. See [`load_pacman`] for the error contract.
pub fn load_package_db(
    env: &Env,
    runner: &dyn crate::command::CommandRunner,
) -> Result<Option<OwnedIndex>, CoreError> {
    let _ = runner;
    match detect_package_db(env) {
        Some(PackageDb::Pacman) => load_pacman(env),
        Some(PackageDb::Dpkg) | Some(PackageDb::Rpm) | None => Ok(None),
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
    /// Paths with glob characters or `%` specifiers, as components; a
    /// specifier is already `*`.
    patterns: Vec<Vec<String>>,
}

impl TmpfilesDeclared {
    /// True when `p` (an absolute path, in practice a direct child of `/etc`
    /// or `/opt`) may hold something declared: a literal path at or below
    /// it, or a pattern at least as deep as `p` whose first components each
    /// glob-match `p`'s (so it could match `p` or something inside it). A
    /// declared ancestor of `p` does not cover it. Components are compared
    /// whole.
    pub fn covers(&self, p: &Path) -> bool {
        let literal = self
            .paths
            .range::<Path, _>((Bound::Included(p), Bound::Unbounded))
            .next()
            .is_some_and(|first| first.starts_with(p));
        if literal {
            return true;
        }
        let Some(comps) = components(p) else {
            return false;
        };
        self.patterns.iter().any(|pat| {
            pat.len() >= comps.len()
                && pat
                    .iter()
                    .zip(&comps)
                    .all(|(g, c)| glob_component(g.as_bytes(), c))
        })
    }
}

/// The normal components of an absolute path, as bytes; `None` for a
/// relative one.
fn components(p: &Path) -> Option<Vec<&[u8]>> {
    use std::os::unix::ffi::OsStrExt;
    if !p.is_absolute() {
        return None;
    }
    Some(
        p.components()
            .filter_map(|c| match c {
                std::path::Component::Normal(n) => Some(n.as_bytes()),
                _ => None,
            })
            .collect(),
    )
}

/// fnmatch-style match of one path component: `*`, `?` and `[...]` (with
/// `!` or `^` negation and `a-z` ranges; a `[` without a closing `]` is
/// literal). Never crosses `/`: both sides are single components.
fn glob_component(pat: &[u8], s: &[u8]) -> bool {
    match pat.split_first() {
        None => s.is_empty(),
        Some((b'*', rest)) => (0..=s.len()).any(|k| glob_component(rest, &s[k..])),
        Some((b'?', rest)) => !s.is_empty() && glob_component(rest, &s[1..]),
        Some((b'[', rest)) => match class(rest) {
            Some((matches, after)) => {
                s.first().is_some_and(|&c| matches(c)) && glob_component(after, &s[1..])
            }
            None => s.first() == Some(&b'[') && glob_component(rest, &s[1..]),
        },
        Some((&c, rest)) => s.first() == Some(&c) && glob_component(rest, &s[1..]),
    }
}

/// A bracket class after its `[`: a matcher and the pattern after the `]`,
/// or `None` when there is no closing `]`.
#[allow(clippy::type_complexity)]
fn class(p: &[u8]) -> Option<(Box<dyn Fn(u8) -> bool + '_>, &[u8])> {
    let (negate, body) = match p.first() {
        Some(b'!' | b'^') => (true, &p[1..]),
        _ => (false, p),
    };
    // A `]` right after the opening is a member, not the end.
    let close = 1 + body.get(1..)?.iter().position(|&b| b == b']')?;
    let set = &body[..close];
    let after = &body[close + 1..];
    let matcher = move |c: u8| {
        let mut i = 0;
        let mut hit = false;
        while i < set.len() {
            if i + 2 < set.len() && set[i + 1] == b'-' {
                hit |= set[i] <= c && c <= set[i + 2];
                i += 3;
            } else {
                hit |= set[i] == c;
                i += 1;
            }
        }
        hit != negate
    };
    Some((Box::new(matcher), after))
}

/// Unquotes a tmpfiles.d path field the way systemd does: a field starting
/// with `"` runs to the closing `"` (whitespace included), and backslash
/// escapes (`\\`, `\"`, `\'`, `\a \b \f \n \r \t \v`, `\xHH`, `\NNN` octal)
/// are decoded. Returns the field and the rest of the line, or `Err(raw)`
/// with the raw field text when it cannot be unquoted.
fn path_field(line: &str) -> Option<Result<String, String>> {
    let line = line.trim_start();
    if line.is_empty() {
        return None;
    }
    let bytes = line.as_bytes();
    let mut out: Vec<u8> = Vec::new();
    let mut i = 0;
    let mut quoted = false;
    let fail = || {
        Some(Err(line
            .split_whitespace()
            .next()
            .unwrap_or("")
            .to_string()))
    };
    while i < bytes.len() {
        let b = bytes[i];
        match b {
            b'"' => {
                quoted = !quoted;
                i += 1;
            }
            b'\\' => {
                let Some(&e) = bytes.get(i + 1) else {
                    return fail();
                };
                let (v, used) = match e {
                    b'\\' | b'"' | b'\'' => (e, 2),
                    b'a' => (7, 2),
                    b'b' => (8, 2),
                    b'f' => (12, 2),
                    b'n' => (b'\n', 2),
                    b'r' => (b'\r', 2),
                    b't' => (b'\t', 2),
                    b'v' => (11, 2),
                    b'x' => match bytes
                        .get(i + 2..i + 4)
                        .and_then(|h| std::str::from_utf8(h).ok())
                        .and_then(|h| u8::from_str_radix(h, 16).ok())
                    {
                        Some(v) => (v, 4),
                        None => return fail(),
                    },
                    b'0'..=b'7' => match bytes
                        .get(i + 1..i + 4)
                        .and_then(|o| std::str::from_utf8(o).ok())
                        .and_then(|o| u8::from_str_radix(o, 8).ok())
                    {
                        Some(v) => (v, 4),
                        None => return fail(),
                    },
                    _ => return fail(),
                };
                out.push(v);
                i += used;
            }
            b' ' | b'\t' if !quoted => break,
            _ => {
                out.push(b);
                i += 1;
            }
        }
    }
    if quoted {
        return fail();
    }
    match String::from_utf8(out) {
        Ok(s) => Some(Ok(s)),
        Err(_) => fail(),
    }
}

/// Whether `p` has a glob character or a `%` specifier.
fn is_pattern(p: &str) -> bool {
    p.contains(['*', '?', '[', '%'])
}

/// A pattern path as components, each `%X` specifier as `*` (`%%` is a
/// literal `%`).
fn pattern_components(p: &str) -> Vec<String> {
    p.split('/')
        .filter(|c| !c.is_empty())
        .map(|c| {
            let mut out = String::new();
            let mut chars = c.chars();
            while let Some(ch) = chars.next() {
                if ch == '%' {
                    match chars.next() {
                        Some('%') => out.push('%'),
                        _ => out.push('*'),
                    }
                } else {
                    out.push(ch);
                }
            }
            out
        })
        .collect()
}

/// For a field that cannot be unquoted: its literal directory prefix (up
/// to the last `/` before the first special character) plus one `*`
/// component, so it covers whatever that directory might hold.
fn fallback_pattern(raw: &str) -> Option<Vec<String>> {
    let raw = raw.trim_start_matches('"');
    if !raw.starts_with('/') {
        return None;
    }
    let first = raw
        .find(['*', '?', '[', '%', '\\', '"'])
        .unwrap_or(raw.len());
    let dir = raw[..first].rfind('/').map_or("", |slash| &raw[..slash]);
    let mut comps = pattern_components(dir);
    comps.push("*".into());
    Some(comps)
}

/// Reads [`TMPFILES_DIRS`], every `*.conf`. The second field of each line
/// is the path, unquoted as systemd does; a leading `%E` is `/etc`. A path
/// with glob characters or `%` specifiers is a pattern
/// ([`TmpfilesDeclared::covers`]); one that cannot be unquoted covers its
/// literal directory conservatively. A path not starting with `/` (another
/// leading specifier) is skipped. A missing directory is fine; an
/// unreadable one is an error.
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
                out.add_line(line);
            }
        }
    }
    Ok(out)
}

impl TmpfilesDeclared {
    /// Adds one `tmpfiles.d` line.
    fn add_line(&mut self, line: &str) {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            return;
        }
        // Skip the type field.
        let Some(rest) = line.split_once([' ', '\t']).map(|(_, r)| r) else {
            return;
        };
        let p = match path_field(rest) {
            None => return,
            Some(Ok(p)) => p,
            Some(Err(raw)) => {
                if let Some(pat) = fallback_pattern(&raw) {
                    self.patterns.push(pat);
                }
                return;
            }
        };
        let p = match p.strip_prefix("%E") {
            Some(rest) if rest.is_empty() || rest.starts_with('/') => format!("/etc{rest}"),
            _ => p,
        };
        if !p.starts_with('/') {
            return;
        }
        if is_pattern(&p) {
            self.patterns.push(pattern_components(&p));
            return;
        }
        let trimmed = p.trim_end_matches('/');
        if !trimmed.is_empty() {
            self.paths.insert(PathBuf::from(trimmed));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::FixtureTree;

    fn declared(lines: &str) -> TmpfilesDeclared {
        let mut d = TmpfilesDeclared::default();
        for l in lines.lines() {
            d.add_line(l);
        }
        d
    }

    #[test]
    fn detect_prefers_pacman_then_dpkg_then_rpm() {
        let t = FixtureTree::new();
        assert_eq!(detect_package_db(&t.env()), None);
        t.write_exec("/usr/bin/rpm", b"#!/bin/sh\n");
        assert_eq!(detect_package_db(&t.env()), Some(PackageDb::Rpm));
        t.write("/var/lib/dpkg/status", b"");
        assert_eq!(detect_package_db(&t.env()), Some(PackageDb::Dpkg));
        t.write("/var/lib/pacman/local/ALPM_DB_VERSION", b"9\n");
        assert_eq!(detect_package_db(&t.env()), Some(PackageDb::Pacman));
    }

    #[test]
    fn load_package_db_dispatches_to_pacman() {
        let t = FixtureTree::new();
        t.write(
            "/var/lib/pacman/local/foo-1-1/files",
            b"%FILES%\nopt/foo/\nopt/foo/bin\n\n",
        );
        let runner = crate::testing::FakeRunner::new();
        let idx = load_package_db(&t.env(), &runner).unwrap().unwrap();
        assert!(idx.owns_at_or_below(Path::new("/opt/foo")));
        assert_eq!(
            load_package_db(&FixtureTree::new().env(), &runner).unwrap(),
            None
        );
    }

    fn covers(d: &TmpfilesDeclared, p: &str) -> bool {
        d.covers(Path::new(p))
    }

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
        for p in [
            "/etc/openvpn",
            "/opt/x",
            "/etc/y",
            "/opt/local-dir",
            "/etc/runtime-dir",
        ] {
            assert!(covers(&got, p), "{p}");
        }
        for p in ["/etc/ignored", "/opt/xy", "/etc/state", "/opt/state"] {
            assert!(!covers(&got, p), "{p}");
        }
        assert_eq!(
            tmpfiles_declared(&FixtureTree::new().env()).unwrap(),
            TmpfilesDeclared::default()
        );
    }

    #[test]
    fn tmpfiles_patterns_cover_only_what_they_can_match() {
        // systemd-nspawn.conf: a pattern in `/` is shallower than any
        // child of /opt.
        let d = declared("R! /.#machine.*\n");
        assert!(!covers(&d, "/opt/inky"));
        assert!(!covers(&d, "/etc/inky"));
        let d = declared("C /etc/tpm2-tss/fapi-profiles/*.json - - - -\n");
        assert!(covers(&d, "/etc/tpm2-tss"));
        assert!(!covers(&d, "/etc/tpm2"));
        let d = declared("d /opt/foo*\n");
        assert!(covers(&d, "/opt/foobar"));
        assert!(covers(&d, "/opt/foo"));
        assert!(!covers(&d, "/opt/bar"));
        let d = declared("x /op*\n");
        assert!(!covers(&d, "/opt/inky"));
        // A declared ancestor covers nothing below it.
        let d = declared("z /\nz /opt\n");
        assert!(!covers(&d, "/opt/inky"));
        assert!(!covers(&d, "/etc/inky"));
        // Specifiers match like `*`, within one component.
        let d = declared("d /opt/inky%b/run\nd /etc/%m.d/x\n");
        assert!(covers(&d, "/opt/inky-1"));
        assert!(!covers(&d, "/opt/other"));
        assert!(covers(&d, "/etc/abc.d"));
        assert!(!covers(&d, "/etc/abc"));
        // Classes, negation and ranges.
        let d = declared("d /opt/tpm[0-9]/x\nd /etc/[!a]x\nd /etc/[^b]y\n");
        assert!(covers(&d, "/opt/tpm3"));
        assert!(!covers(&d, "/opt/tpmx"));
        assert!(covers(&d, "/etc/bx") && !covers(&d, "/etc/ax"));
        assert!(covers(&d, "/etc/ay") && !covers(&d, "/etc/by"));
        let d = declared("d /opt/a?c\n");
        assert!(covers(&d, "/opt/abc") && !covers(&d, "/opt/ac"));
    }

    #[test]
    fn tmpfiles_specifier_e_and_quoting() {
        let d = declared("d %E/inky 0755\n");
        assert!(covers(&d, "/etc/inky"));
        let d = declared("d \"/opt/foo bar\" 0755\n");
        assert!(covers(&d, "/opt/foo bar"));
        assert!(!covers(&d, "/opt/foo"));
        let d = declared("d /opt/a\\x20b 0755\n");
        assert!(covers(&d, "/opt/a b"));
        // Cannot be unquoted: covers everything its literal directory may
        // hold.
        let d = declared("d \"/opt/broken 0755\n");
        assert!(covers(&d, "/opt/inky"));
        assert!(!covers(&d, "/etc/inky"));
        let d = declared("d /etc/x/y\\q 0755\n");
        assert!(covers(&d, "/etc/x"));
        assert!(!covers(&d, "/etc/inky"));
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
