//! The gate every destructive action passes through. All checks fail closed.
//!
//! `verify` only proves the state at the moment it is called. Callers must act
//! on exactly the path they verified, using no-follow operations. `verify` and
//! `fingerprint` refuse non-canonical paths (trailing `/`, `.`, `//`, `..`)
//! because the kernel resolves `link/` and `link/.` through the symlink.

use std::fs;
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::config::{Config, glob_match};
use crate::env::{Env, display_path};
use crate::error::CoreError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Protection {
    /// Only the path itself is protected; children may be acted on.
    Exact,
    /// The path and everything below it is protected.
    Subtree,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum SafetyError {
    #[error("refusing: {0} is protected")]
    Protected(String),
    #[error("refusing: {0} is outside the allowed roots")]
    OutsideAllowedRoots(String),
    #[error("refusing: {0} changed since it was scanned")]
    Changed(String),
    #[error("refusing: {0} is not a canonical path")]
    NonCanonical(String),
    #[error("{0} no longer exists")]
    Missing(String),
}

/// Built-in protected paths (spec §8) plus `cfg.protect` as subtrees, all
/// in-system.
pub fn protected_paths(env: &Env, cfg: &Config) -> Vec<(PathBuf, Protection)> {
    let home = env.unsys(&env.home);
    let mut v: Vec<(PathBuf, Protection)> = Vec::new();
    v.push((home.clone(), Protection::Exact));
    for p in [
        &env.config_home,
        &env.cache_home,
        &env.data_home,
        &env.state_home,
    ] {
        v.push((env.unsys(p), Protection::Exact));
    }
    for p in ["/", "/etc", "/opt", "/var", "/home"] {
        v.push((PathBuf::from(p), Protection::Exact));
    }
    for rel in [
        ".ssh",
        ".gnupg",
        ".pki",
        ".local/share/keyrings",
        ".password-store",
        ".config/crumbs",
        ".local/state/crumbs",
        "Documents",
        "Desktop",
        "Pictures",
        "Music",
        "Videos",
    ] {
        v.push((home.join(rel), Protection::Subtree));
    }
    // The same, wherever the XDG variables point.
    for (base, rel) in [
        (&env.config_home, "crumbs"),
        (&env.state_home, "crumbs"),
        (&env.data_home, "keyrings"),
    ] {
        v.push((env.unsys(base).join(rel), Protection::Subtree));
    }
    for p in [
        "/usr",
        "/boot",
        "/var/lib/pacman",
        "/var/lib/dpkg",
        "/var/lib/rpm",
    ] {
        v.push((PathBuf::from(p), Protection::Subtree));
    }
    for p in &cfg.protect {
        v.push((p.clone(), Protection::Subtree));
    }
    v
}

/// Lexical normalisation: drops `.` components. Returns `None` for relative
/// paths and for any path containing `..` (callers treat that as unsafe).
fn normalise(p: &Path) -> Option<PathBuf> {
    if !p.is_absolute() {
        return None;
    }
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => return None,
            other => out.push(other.as_os_str()),
        }
    }
    Some(out)
}

pub fn is_protected(env: &Env, cfg: &Config, in_system: &Path) -> bool {
    is_protected_by(&protected_paths(env, cfg), in_system)
}

/// [`is_protected`] against a precomputed [`protected_paths`] list.
pub(crate) fn is_protected_by(protected: &[(PathBuf, Protection)], in_system: &Path) -> bool {
    let Some(path) = normalise(in_system) else {
        return true;
    };
    protected.iter().any(|(p, kind)| {
        let Some(p) = normalise(p) else {
            // A malformed configured entry can't be compared: fail closed.
            return true;
        };
        match kind {
            Protection::Exact => path == p || p.starts_with(&path),
            Protection::Subtree => path.starts_with(&p) || p.starts_with(&path),
        }
    })
}

pub fn is_allowed_user_target(env: &Env, cfg: &Config, in_system: &Path) -> bool {
    is_allowed_user_target_by(env, cfg, &protected_paths(env, cfg), in_system)
}

/// [`is_allowed_user_target`] against a precomputed [`protected_paths`] list.
pub(crate) fn is_allowed_user_target_by(
    env: &Env,
    cfg: &Config,
    protected: &[(PathBuf, Protection)],
    in_system: &Path,
) -> bool {
    let Some(path) = normalise(in_system) else {
        return false;
    };
    let home = env.unsys(&env.home);
    if path == home || !path.starts_with(&home) {
        return false;
    }
    if is_protected_by(protected, &path) {
        return false;
    }
    let text = path.to_string_lossy();
    !cfg.exclude.iter().any(|pat| {
        // Like a protected path, an exclude also shields every ancestor of
        // its literal part: trashing `~/.config/Foo` would take the excluded
        // `~/.config/Foo/keep` with it.
        glob_match(pat, &text) || Path::new(literal_prefix(pat)).starts_with(&path)
    })
}

/// The part of an exclude pattern before any wildcard: up to the last `/`
/// before the first `*`, or the whole pattern (without a trailing `/`)
/// when it has none.
fn literal_prefix(pattern: &str) -> &str {
    match pattern.find('*') {
        Some(star) => pattern[..star].rfind('/').map_or("", |i| &pattern[..i]),
        None if pattern.len() > 1 => pattern.trim_end_matches('/'),
        None => pattern,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fingerprint {
    pub dev: u64,
    pub ino: u64,
    pub is_symlink: bool,
}

/// True when the path is absolute and already in normalised form.
pub(crate) fn is_canonical(p: &Path) -> bool {
    normalise(p).is_some_and(|n| n.as_os_str() == p.as_os_str())
}

fn real_path(env: &Env, in_system: &Path) -> PathBuf {
    match in_system.strip_prefix("/") {
        Ok(rest) => env.root.join(rest),
        Err(_) => env.root.join(in_system),
    }
}

fn lstat(env: &Env, in_system: &Path) -> io::Result<Fingerprint> {
    let m = fs::symlink_metadata(real_path(env, in_system))?;
    Ok(Fingerprint {
        dev: m.dev(),
        ino: m.ino(),
        is_symlink: m.file_type().is_symlink(),
    })
}

/// Fails with `CoreError::Io` (`InvalidInput`) for non-canonical paths.
pub fn fingerprint(env: &Env, in_system: &Path) -> Result<Fingerprint, CoreError> {
    if !is_canonical(in_system) {
        return Err(CoreError::Io {
            path: in_system.to_path_buf(),
            source: io::Error::new(io::ErrorKind::InvalidInput, "path is not canonical"),
        });
    }
    lstat(env, in_system).map_err(|source| CoreError::Io {
        path: in_system.to_path_buf(),
        source,
    })
}

/// Re-checks `in_system` immediately before an action. Order: allowed target,
/// parent chain (no symlinks between home and the path), then the target's own
/// `(dev, ino, is_symlink)` against `expected`.
pub fn verify(
    env: &Env,
    cfg: &Config,
    in_system: &Path,
    expected: &Fingerprint,
) -> Result<(), SafetyError> {
    let shown = display_path(env, in_system);
    if !is_canonical(in_system) {
        return Err(SafetyError::NonCanonical(shown));
    }
    if is_protected(env, cfg, in_system) {
        return Err(SafetyError::Protected(shown));
    }
    if !is_allowed_user_target(env, cfg, in_system) {
        return Err(SafetyError::OutsideAllowedRoots(shown));
    }
    // Allowed implies normalisable and strictly under home.
    let (Some(path), home) = (normalise(in_system), env.unsys(&env.home)) else {
        return Err(SafetyError::OutsideAllowedRoots(shown));
    };
    let Some(parent) = path.parent() else {
        return Err(SafetyError::OutsideAllowedRoots(shown));
    };
    // Every directory strictly below home, down to the parent of the target.
    let mut dir = home.clone();
    let rel_parent = parent.strip_prefix(&home).unwrap_or(Path::new(""));
    for comp in rel_parent.components() {
        dir.push(comp.as_os_str());
        match lstat(env, &dir) {
            Ok(fp) if !fp.is_symlink => {}
            Ok(_) => return Err(SafetyError::Changed(shown)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                return Err(SafetyError::Missing(shown));
            }
            Err(_) => return Err(SafetyError::Changed(shown)),
        }
    }
    match lstat(env, &path) {
        Ok(fp) if fp == *expected => Ok(()),
        Ok(_) => Err(SafetyError::Changed(shown)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Err(SafetyError::Missing(shown)),
        Err(_) => Err(SafetyError::Changed(shown)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::FixtureTree;

    fn setup() -> (FixtureTree, Env, Config) {
        let t = FixtureTree::new();
        let env = t.env();
        (t, env, Config::default())
    }

    fn p(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    #[test]
    fn home_itself_protected_exact_child_allowed() {
        let (_t, env, cfg) = setup();
        assert!(is_protected(&env, &cfg, &p("/home/user")));
        assert!(!is_allowed_user_target(&env, &cfg, &p("/home/user")));
        assert!(is_protected(&env, &cfg, &p("/home/user/.config")));
        assert!(!is_protected(&env, &cfg, &p("/home/user/.config/foo")));
        assert!(is_allowed_user_target(
            &env,
            &cfg,
            &p("/home/user/.config/foo")
        ));
    }

    #[test]
    fn ssh_subtree_protected() {
        let (_t, env, cfg) = setup();
        assert!(is_protected(&env, &cfg, &p("/home/user/.ssh/known_hosts")));
        assert!(is_protected(&env, &cfg, &p("/home/user/Documents/a/b")));
        assert!(!is_protected(&env, &cfg, &p("/home/user/.sshx")));
    }

    #[test]
    fn ancestor_of_protected_is_protected() {
        let (_t, env, cfg) = setup();
        assert!(is_protected(&env, &cfg, &p("/home/user/.local")));
        assert!(is_protected(&env, &cfg, &p("/home/user/.local/share")));
        assert!(!is_protected(&env, &cfg, &p("/home/user/.local/share/foo")));
        assert!(is_protected(&env, &cfg, &p("/var/lib")));
    }

    #[test]
    fn custom_xdg_crumbs_and_keyrings_protected() {
        let (_t, mut env, cfg) = setup();
        env.config_home = env.home.join("xdg/config");
        env.state_home = env.home.join("xdg/state");
        env.data_home = env.home.join("xdg/data");
        for s in [
            "/home/user/xdg/config/crumbs",
            "/home/user/xdg/config/crumbs/config.toml",
            "/home/user/xdg/state/crumbs/history.jsonl",
            "/home/user/xdg/data/keyrings/login.keyring",
            // The ~ forms stay protected too.
            "/home/user/.config/crumbs",
            "/home/user/.local/state/crumbs",
            "/home/user/.local/share/keyrings",
        ] {
            assert!(is_protected(&env, &cfg, &p(s)), "{s}");
            assert!(!is_allowed_user_target(&env, &cfg, &p(s)), "{s}");
        }
        assert!(is_allowed_user_target(
            &env,
            &cfg,
            &p("/home/user/xdg/config/other")
        ));
    }

    #[test]
    fn system_paths_protected() {
        let (_t, env, cfg) = setup();
        for s in [
            "/",
            "/etc",
            "/opt",
            "/var",
            "/home",
            "/usr/bin/ls",
            "/boot/x",
        ] {
            assert!(is_protected(&env, &cfg, &p(s)), "{s}");
        }
        assert!(!is_protected(&env, &cfg, &p("/opt/foo")));
    }

    #[test]
    fn user_protect_added() {
        let (_t, env, mut cfg) = setup();
        let target = p("/home/user/.config/keepme/sub");
        assert!(is_allowed_user_target(&env, &cfg, &target));
        cfg.protect.push(p("/home/user/.config/keepme"));
        assert!(is_protected(&env, &cfg, &target));
        assert!(!is_allowed_user_target(&env, &cfg, &target));
    }

    #[test]
    fn exclude_glob_blocks_target() {
        let (_t, env, mut cfg) = setup();
        cfg.exclude.push("/home/user/.config/*-keep".into());
        assert!(!is_allowed_user_target(
            &env,
            &cfg,
            &p("/home/user/.config/a-keep/x")
        ));
        assert!(is_allowed_user_target(
            &env,
            &cfg,
            &p("/home/user/.config/a-drop")
        ));
    }

    #[test]
    fn exclude_protects_ancestors_of_its_literal_prefix() {
        let (_t, env, mut cfg) = setup();
        cfg.exclude.push("/home/user/.config/Foobar/keep".into());
        cfg.exclude
            .push("/home/user/.local/share/Baz/sub/*.db".into());
        for s in [
            "/home/user/.config/Foobar/keep",
            "/home/user/.config/Foobar/keep/important",
            "/home/user/.config/Foobar",
            "/home/user/.local/share/Baz/sub",
            "/home/user/.local/share/Baz",
            "/home/user/.local/share/Baz/sub/x.db",
        ] {
            assert!(!is_allowed_user_target(&env, &cfg, &p(s)), "{s}");
        }
        for s in [
            "/home/user/.config/Foobar/other",
            "/home/user/.config/Foobarx",
            "/home/user/.local/share/Baz/sub/x.txt",
            "/home/user/.local/share/Baz/other",
        ] {
            assert!(is_allowed_user_target(&env, &cfg, &p(s)), "{s}");
        }
    }

    #[test]
    fn outside_home_not_allowed() {
        let (_t, env, cfg) = setup();
        assert!(!is_allowed_user_target(&env, &cfg, &p("/opt/foo")));
        assert!(!is_allowed_user_target(&env, &cfg, &p("/tmp/x")));
        assert!(!is_allowed_user_target(&env, &cfg, &p("/home/username/x")));
    }

    #[test]
    fn dotdot_and_relative_fail_closed() {
        let (_t, env, cfg) = setup();
        assert!(is_protected(
            &env,
            &cfg,
            &p("/home/user/.config/foo/../../.ssh")
        ));
        assert!(!is_allowed_user_target(
            &env,
            &cfg,
            &p("/home/user/.config/foo/../bar")
        ));
        assert!(!is_allowed_user_target(&env, &cfg, &p("foo")));
        // `.` components are dropped, so these compare normally.
        assert!(is_protected(&env, &cfg, &p("/home/user/./.ssh/x")));
        assert!(is_allowed_user_target(
            &env,
            &cfg,
            &p("/home/user/./.config/foo")
        ));
    }

    #[test]
    fn verify_ok_when_unchanged() {
        let (t, env, cfg) = setup();
        t.mkdir("home/user/.config/foo");
        let target = p("/home/user/.config/foo");
        let fp = fingerprint(&env, &target).unwrap();
        assert!(!fp.is_symlink);
        assert_eq!(verify(&env, &cfg, &target, &fp), Ok(()));
    }

    #[test]
    fn verify_detects_swap() {
        let (t, env, cfg) = setup();
        t.mkdir("home/user/.config/foo");
        t.mkdir("home/user/Documents");
        let target = p("/home/user/.config/foo");
        let fp = fingerprint(&env, &target).unwrap();
        std::fs::remove_dir(t.dir.path().join("home/user/.config/foo")).unwrap();
        t.symlink("home/user/.config/foo", "/home/user/Documents");
        assert!(matches!(
            verify(&env, &cfg, &target, &fp),
            Err(SafetyError::Changed(_))
        ));
    }

    #[test]
    fn verify_detects_symlinked_parent() {
        let (t, env, cfg) = setup();
        t.write("home/user/.config/foo/bar", b"x");
        t.write("home/user/elsewhere/foo/bar", b"x");
        let target = p("/home/user/.config/foo/bar");
        let fp = fingerprint(&env, &target).unwrap();
        std::fs::remove_dir_all(t.dir.path().join("home/user/.config")).unwrap();
        t.symlink("home/user/.config", "/home/user/elsewhere");
        assert!(matches!(
            verify(&env, &cfg, &target, &fp),
            Err(SafetyError::Changed(_))
        ));
    }

    #[test]
    fn verify_missing() {
        let (t, env, cfg) = setup();
        t.write("home/user/.config/foo/bar", b"x");
        let target = p("/home/user/.config/foo/bar");
        let fp = fingerprint(&env, &target).unwrap();
        std::fs::remove_file(t.dir.path().join("home/user/.config/foo/bar")).unwrap();
        assert!(matches!(
            verify(&env, &cfg, &target, &fp),
            Err(SafetyError::Missing(_))
        ));
    }

    #[test]
    fn verify_refuses_protected_and_outside() {
        let (t, env, cfg) = setup();
        t.write("home/user/.ssh/id", b"k");
        t.write("opt/foo", b"x");
        let ssh = p("/home/user/.ssh/id");
        let fp = fingerprint(&env, &ssh).unwrap();
        assert!(matches!(
            verify(&env, &cfg, &ssh, &fp),
            Err(SafetyError::Protected(_))
        ));
        let out = p("/tmp/x");
        t.write("tmp/x", b"x");
        let fp = fingerprint(&env, &out).unwrap();
        assert!(matches!(
            verify(&env, &cfg, &out, &fp),
            Err(SafetyError::OutsideAllowedRoots(_))
        ));
    }

    #[test]
    fn verify_refuses_non_canonical_symlink_spellings() {
        let (t, env, cfg) = setup();
        t.mkdir("home/user/Documents");
        t.symlink("home/user/.config/foo", "/home/user/Documents");
        let link = p("/home/user/.config/foo");
        let fp = fingerprint(&env, &link).unwrap();
        assert!(fp.is_symlink);
        assert_eq!(verify(&env, &cfg, &link, &fp), Ok(()));
        for s in [
            "/home/user/.config/foo/",
            "/home/user/.config/foo/.",
            "/home/user/.config//foo",
        ] {
            assert!(
                matches!(
                    verify(&env, &cfg, &p(s), &fp),
                    Err(SafetyError::NonCanonical(_))
                ),
                "{s}"
            );
        }
    }

    #[test]
    fn fingerprint_refuses_non_canonical() {
        let (t, env, _cfg) = setup();
        t.mkdir("home/user/.config/foo");
        for s in [
            "/home/user/.config/foo/",
            "/home/user/.config/foo/.",
            "/home/user/.config/../.config/foo",
        ] {
            assert!(fingerprint(&env, &p(s)).is_err(), "{s}");
        }
        assert!(fingerprint(&env, &p("/home/user/.config/foo")).is_ok());
    }
}
