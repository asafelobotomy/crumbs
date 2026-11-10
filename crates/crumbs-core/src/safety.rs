//! The gate every destructive action passes through. All checks fail closed.
//!
//! `verify` only proves the state at the moment it is called. Callers must act
//! on exactly the path they verified, using no-follow operations. `verify` and
//! `fingerprint` refuse non-canonical paths (trailing `/`, `.`, `//`, `..`)
//! because the kernel resolves `link/` and `link/.` through the symlink.

use std::ffi::OsStr;
use std::fs;
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::config::{Config, could_match_below, glob_match};
use crate::env::{Env, display_path};
use crate::error::CoreError;
use crate::trash::validate_private_dir;

/// Where a target may lie, and so which root its parent chain is checked
/// from. Every scope also applies the protected paths and `exclude`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Scope {
    /// Strictly under the home directory.
    #[default]
    Home,
    /// A direct child of `/tmp` or `/var/tmp`: owned by the user, a regular
    /// file or a directory, not a symlink; its parent root-owned and sticky.
    UserTemp,
    /// A direct child of `{data_home}/Trash/files` or `{data_home}/Trash/info`,
    /// with `Trash`, `files` and `info` private to the user.
    HomeTrash,
    /// A root-owned system file, deleted with root rights (coredumps only
    /// in M4): a direct child regular file of `/var/lib/systemd/coredump`
    /// named `core.*`, with every directory from `/` down to that one a
    /// real directory owned by [`Env::system_owner_uid`] and writable by
    /// no one else.
    System,
    /// A direct child of `/etc` or `/opt` that no package owns, removed
    /// with root rights: a real directory or a regular file, never a
    /// symlink, with every directory from `/` down to `/etc` or `/opt` a
    /// real directory owned by [`Env::system_owner_uid`] and writable by no
    /// one else.
    SystemTree,
}

/// Names under `/etc` and `/opt` that are never offered for removal even
/// when no package owns them: system configuration that crumbs must leave
/// alone.
pub const ETC_NEVER: &[&str] = &[
    "passwd",
    "shadow",
    "group",
    "gshadow",
    "sudoers",
    "sudoers.d",
    "ssh",
    "ssl",
    "ca-certificates",
    "pam.d",
    "security",
    "systemd",
    "fstab",
    "crypttab",
    "hosts",
    "hostname",
    "machine-id",
    "locale.conf",
    "localtime",
    "vconsole.conf",
    "adjtime",
    "ld.so.cache",
    "ld.so.conf",
    "ld.so.conf.d",
    "os-release",
    "pacman.conf",
    "pacman.d",
    "mkinitcpio.conf",
    "mkinitcpio.d",
    "modprobe.d",
    "modules-load.d",
    "sysctl.d",
    "udev",
    "X11",
    "NetworkManager",
    "polkit-1",
    "dbus-1",
    "default",
    "skel",
    "profile",
    "profile.d",
    "environment",
    "subuid",
    "subgid",
    "kernel",
    "cmdline.d",
    "secureboot",
    "crypttab.initramfs",
    "machine-info",
    "resolv.conf",
    "nsswitch.conf",
    "shells",
    "motd",
    "opt",
    "securetty",
    "dconf",
    "issue",
    "issue.net",
    "timezone",
    "login.defs",
    "apt",
    "dpkg",
    "dnf",
    "yum.repos.d",
    "rpm",
    "sysconfig",
    "alternatives",
    "selinux",
    "sudo.conf",
];

/// Whether `name` may be a [`Scope::SystemTree`] target: not in
/// [`ETC_NEVER`], not a backup or package-manager leftover suffix, and not
/// hidden.
pub fn is_system_leftover_name(name: &OsStr) -> bool {
    let b = name.as_bytes();
    if b.is_empty() || b.starts_with(b".") || b.ends_with(b"-") {
        return false;
    }
    if [&b".bak"[..], b".old", b".pacnew", b".pacsave"]
        .iter()
        .any(|suffix| b.ends_with(suffix))
    {
        return false;
    }
    !ETC_NEVER.iter().any(|n| n.as_bytes() == b)
}

/// In-system directory whose `core.*` files [`Scope::System`] allows.
const COREDUMP_DIR: &str = "/var/lib/systemd/coredump";

impl Scope {
    pub fn is_home(&self) -> bool {
        *self == Scope::Home
    }
}

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
    /// A directory between the scope's root and the target, or the root
    /// itself outside `Home`, is a symlink. Holds the display form of that
    /// directory (`~/…` under home).
    #[error("refusing: {} is a symlink", .0.display())]
    SymlinkedParent(PathBuf),
    /// The scope's rules reject the target or its root directory: the
    /// display path and why (`is not owned by you`, …).
    #[error("refusing: {0} {1}")]
    Ineligible(String, &'static str),
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

/// Whether `in_system` lies where `scope` allows and is neither protected
/// nor excluded. Purely lexical; [`verify`] adds the on-disk checks.
pub fn is_allowed_target(env: &Env, cfg: &Config, scope: Scope, in_system: &Path) -> bool {
    let protected = protected_paths(env, cfg);
    let Some(path) = normalise(in_system) else {
        return false;
    };
    match scope {
        Scope::Home => is_allowed_user_target_by(env, cfg, &protected, &path),
        Scope::UserTemp | Scope::HomeTrash => {
            let home = env.unsys(&env.home);
            // `/tmp` children never reach into home, whatever `HOME` is.
            let in_root = path
                .parent()
                .is_some_and(|parent| scope_roots(env, scope).iter().any(|root| parent == root))
                && (scope == Scope::HomeTrash || !path.starts_with(&home));
            in_root && !is_protected_by(&protected, &path) && !is_excluded(cfg, &path)
        }
        Scope::System => {
            let is_core = path
                .file_name()
                .is_some_and(|n| n.as_bytes().len() > 5 && n.as_bytes().starts_with(b"core."));
            is_core
                && path.parent() == Some(Path::new(COREDUMP_DIR))
                && !is_protected_by(&protected, &path)
                && !is_excluded(cfg, &path)
        }
        Scope::SystemTree => {
            let in_root = path
                .parent()
                .is_some_and(|parent| scope_roots(env, scope).iter().any(|root| parent == root));
            in_root
                && path.file_name().is_some_and(is_system_leftover_name)
                && !is_protected_by(&protected, &path)
                && !is_excluded(cfg, &path)
        }
    }
}

/// The in-system directories whose direct children `scope` allows. Empty
/// for `Home`, whose targets may lie anywhere below home.
fn scope_roots(env: &Env, scope: Scope) -> Vec<PathBuf> {
    match scope {
        Scope::Home => Vec::new(),
        Scope::UserTemp => vec![PathBuf::from("/tmp"), PathBuf::from("/var/tmp")],
        Scope::HomeTrash => {
            let trash = env.unsys(&env.data_home).join("Trash");
            // A `data_home` outside home must not make the trash a way to
            // reach other places.
            if !trash.starts_with(env.unsys(&env.home)) {
                return Vec::new();
            }
            vec![trash.join("files"), trash.join("info")]
        }
        Scope::System => vec![PathBuf::from(COREDUMP_DIR)],
        Scope::SystemTree => vec![PathBuf::from("/etc"), PathBuf::from("/opt")],
    }
}

/// The [`Scope::Home`] case of [`is_allowed_target`].
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
    !is_protected_by(protected, &path) && !is_excluded(cfg, &path)
}

/// Whether an `exclude` pattern matches `path` or could match below it.
fn is_excluded(cfg: &Config, path: &Path) -> bool {
    let text = path.to_string_lossy();
    // Like a protected path, an exclude also shields every ancestor that
    // could contain a match: trashing `~/.config/Foo` would take an
    // excluded `~/.config/Foo/keep` (or `~/.config/*/keep`) with it.
    cfg.exclude
        .iter()
        .any(|pat| glob_match(pat, &text) || could_match_below(pat, &text))
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Fingerprint {
    pub dev: u64,
    pub ino: u64,
    pub is_symlink: bool,
    /// Status-change time of a file or symlink: a file replaced by another
    /// that reuses the inode still gets a new ctime. Always 0 for a
    /// directory, whose ctime changes whenever an entry is added or removed.
    pub ctime: i64,
    pub ctime_nsec: i64,
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
    fs::symlink_metadata(real_path(env, in_system)).map(|m| fingerprint_of(&m))
}

fn fingerprint_of(m: &fs::Metadata) -> Fingerprint {
    Fingerprint {
        dev: m.dev(),
        ino: m.ino(),
        is_symlink: m.file_type().is_symlink(),
        ctime: if m.is_dir() { 0 } else { m.ctime() },
        ctime_nsec: if m.is_dir() { 0 } else { m.ctime_nsec() },
    }
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

/// Re-checks `in_system` immediately before an action. Order: allowed
/// target for `scope`, parent chain (no symlinks from the scope's root down
/// to the path's parent), the scope's own rules for the target, then the
/// target's `(dev, ino, is_symlink, ctime)` against `expected`.
pub fn verify(
    env: &Env,
    cfg: &Config,
    scope: Scope,
    in_system: &Path,
    expected: &Fingerprint,
) -> Result<(), SafetyError> {
    let fp = verify_now(env, cfg, scope, in_system)?;
    if fp == *expected {
        Ok(())
    } else {
        Err(SafetyError::Changed(display_path(env, in_system)))
    }
}

/// The checks of [`verify`] for a path that has no scan-time fingerprint
/// yet: whether it could be acted on right now. Returns its current
/// fingerprint.
pub fn verify_now(
    env: &Env,
    cfg: &Config,
    scope: Scope,
    in_system: &Path,
) -> Result<Fingerprint, SafetyError> {
    let path = check_location(env, cfg, scope, in_system)?;
    let shown = display_path(env, in_system);
    let meta = match fs::symlink_metadata(real_path(env, &path)) {
        Ok(m) => m,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Err(SafetyError::Missing(shown)),
        Err(_) => return Err(SafetyError::Changed(shown)),
    };
    if scope == Scope::UserTemp {
        // Root could remove other users' files; temp cleanup is per user.
        if env.uid == 0 {
            return Err(SafetyError::Ineligible(
                shown,
                "temp cleanup is not available as root",
            ));
        }
        let ft = meta.file_type();
        if !(ft.is_file() || ft.is_dir()) {
            return Err(SafetyError::Ineligible(
                shown,
                "is not a regular file or directory",
            ));
        }
        if meta.uid() != env.uid {
            return Err(SafetyError::Ineligible(shown, "is not owned by you"));
        }
    }
    if scope == Scope::System {
        let ft = meta.file_type();
        if ft.is_symlink() {
            return Err(SafetyError::Ineligible(shown, "is a symlink"));
        }
        if !ft.is_file() {
            return Err(SafetyError::Ineligible(shown, "is not a regular file"));
        }
    } else if scope == Scope::SystemTree {
        let ft = meta.file_type();
        if ft.is_symlink() {
            return Err(SafetyError::Ineligible(shown, "is a symlink"));
        }
        if !(ft.is_file() || ft.is_dir()) {
            return Err(SafetyError::Ineligible(
                shown,
                "is not a regular file or directory",
            ));
        }
    }
    Ok(fingerprint_of(&meta))
}

/// Everything [`verify`] checks before looking at the target itself:
/// canonical, allowed for `scope`, and its root and parent chain intact.
/// Returns the normalised path.
fn check_location(
    env: &Env,
    cfg: &Config,
    scope: Scope,
    in_system: &Path,
) -> Result<PathBuf, SafetyError> {
    let shown = display_path(env, in_system);
    if !is_canonical(in_system) {
        return Err(SafetyError::NonCanonical(shown));
    }
    if is_protected(env, cfg, in_system) {
        return Err(SafetyError::Protected(shown));
    }
    if !is_allowed_target(env, cfg, scope, in_system) {
        return Err(SafetyError::OutsideAllowedRoots(shown));
    }
    // Allowed implies normalisable with a parent.
    let Some(path) = normalise(in_system) else {
        return Err(SafetyError::OutsideAllowedRoots(shown));
    };
    let Some(parent) = path.parent() else {
        return Err(SafetyError::OutsideAllowedRoots(shown));
    };
    match scope {
        Scope::Home => check_parent_chain(env, &env.unsys(&env.home), parent, &shown)?,
        Scope::UserTemp => {
            // `/tmp` itself may be a mount, but never a symlink.
            check_real_dir(env, parent, &shown)?;
            let safe =
                fs::symlink_metadata(real_path(env, parent)).is_ok_and(|m| is_safe_tmp(env, &m));
            if !safe {
                return Err(SafetyError::Ineligible(
                    display_path(env, parent),
                    "is not root-owned with the sticky bit",
                ));
            }
        }
        Scope::HomeTrash => {
            let trash = env.unsys(&env.data_home).join("Trash");
            for dir in [trash.clone(), trash.join("files"), trash.join("info")] {
                check_real_dir(env, &dir, &shown)?;
                if validate_private_dir(env, &real_path(env, &dir)).is_err() {
                    return Err(SafetyError::Ineligible(
                        display_path(env, &dir),
                        "is not a private directory owned by you",
                    ));
                }
            }
        }
        Scope::System | Scope::SystemTree => check_system_chain(env, parent, &shown)?,
    }
    Ok(path)
}

/// Every directory from `/` down to and including `parent` is a real
/// directory (no symlink) owned by [`Env::system_owner_uid`] and not
/// writable by group or others: nobody but root can swap anything on the
/// way to a root-privileged delete. Fails closed on any error.
fn check_system_chain(env: &Env, parent: &Path, shown: &str) -> Result<(), SafetyError> {
    let mut dir = PathBuf::from("/");
    let comps = std::iter::once(None).chain(
        parent
            .components()
            .filter(|c| matches!(c, Component::Normal(_)))
            .map(Some),
    );
    for comp in comps {
        if let Some(c) = comp {
            dir.push(c.as_os_str());
        }
        check_real_dir(env, &dir, shown)?;
        let Ok(m) = fs::symlink_metadata(real_path(env, &dir)) else {
            return Err(SafetyError::Changed(shown.to_string()));
        };
        if m.uid() != env.system_owner_uid {
            return Err(SafetyError::Ineligible(
                display_path(env, &dir),
                "is not owned by root",
            ));
        }
        if m.mode() & 0o022 != 0 {
            return Err(SafetyError::Ineligible(
                display_path(env, &dir),
                "is writable by group or others",
            ));
        }
    }
    Ok(())
}

/// Whether in-system `/tmp` (its lstat data) is safe to act in: owned by
/// [`Env::system_owner_uid`] (root on the real system) and sticky, so nobody
/// else can rename or replace the user's entries in it.
pub(crate) fn is_safe_tmp(env: &Env, m: &fs::Metadata) -> bool {
    m.uid() == env.system_owner_uid && m.mode() & 0o1000 != 0
}

/// No directory strictly below `root`, down to and including `parent`, is
/// a symlink. `shown` names the target in errors.
fn check_parent_chain(
    env: &Env,
    root: &Path,
    parent: &Path,
    shown: &str,
) -> Result<(), SafetyError> {
    let mut dir = root.to_path_buf();
    let rel_parent = parent.strip_prefix(root).unwrap_or(Path::new(""));
    for comp in rel_parent.components() {
        dir.push(comp.as_os_str());
        check_real_dir(env, &dir, shown)?;
    }
    Ok(())
}

/// `dir` exists and is a real directory, not a symlink. `shown` names the
/// target in errors.
fn check_real_dir(env: &Env, dir: &Path, shown: &str) -> Result<(), SafetyError> {
    match fs::symlink_metadata(real_path(env, dir)) {
        Ok(m) if m.file_type().is_symlink() => Err(SafetyError::SymlinkedParent(PathBuf::from(
            display_path(env, dir),
        ))),
        Ok(m) if m.is_dir() => Ok(()),
        Ok(_) => Err(SafetyError::Changed(shown.to_string())),
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            Err(SafetyError::Missing(shown.to_string()))
        }
        Err(_) => Err(SafetyError::Changed(shown.to_string())),
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
    fn exclude_wildcard_shields_parent() {
        let (_t, env, mut cfg) = setup();
        cfg.exclude.push("/home/user/.config/*/keep".into());
        for s in [
            "/home/user/.config/Foobar",
            "/home/user/.config",
            "/home/user/.config/Foobar/keep",
        ] {
            assert!(!is_allowed_user_target(&env, &cfg, &p(s)), "{s}");
        }
        assert!(is_allowed_user_target(
            &env,
            &cfg,
            &p("/home/user/.cache/x")
        ));
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
        assert_eq!(verify(&env, &cfg, Scope::Home, &target, &fp), Ok(()));
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
            verify(&env, &cfg, Scope::Home, &target, &fp),
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
        let err = verify(&env, &cfg, Scope::Home, &target, &fp).unwrap_err();
        assert_eq!(
            err,
            SafetyError::SymlinkedParent(PathBuf::from("~/.config"))
        );
        assert_eq!(err.to_string(), "refusing: ~/.config is a symlink");
    }

    #[test]
    fn verify_now_checks_location_and_existence() {
        let (t, env, cfg) = setup();
        t.write("home/user/Applications/X.AppImage", b"x");
        t.write("opt/apps/Y.AppImage", b"y");
        let ok = p("/home/user/Applications/X.AppImage");
        assert_eq!(
            verify_now(&env, &cfg, Scope::Home, &ok),
            Ok(fingerprint(&env, &ok).unwrap())
        );
        assert!(matches!(
            verify_now(&env, &cfg, Scope::Home, &p("/opt/apps/Y.AppImage")),
            Err(SafetyError::OutsideAllowedRoots(_))
        ));
        assert!(matches!(
            verify_now(
                &env,
                &cfg,
                Scope::Home,
                &p("/home/user/Applications/Z.AppImage")
            ),
            Err(SafetyError::Missing(_))
        ));
        std::fs::rename(
            t.dir.path().join("home/user/Applications"),
            t.dir.path().join("home/user/real"),
        )
        .unwrap();
        t.symlink("home/user/Applications", "real");
        assert_eq!(
            verify_now(&env, &cfg, Scope::Home, &ok),
            Err(SafetyError::SymlinkedParent(p("~/Applications")))
        );
    }

    #[test]
    fn verify_missing() {
        let (t, env, cfg) = setup();
        t.write("home/user/.config/foo/bar", b"x");
        let target = p("/home/user/.config/foo/bar");
        let fp = fingerprint(&env, &target).unwrap();
        std::fs::remove_file(t.dir.path().join("home/user/.config/foo/bar")).unwrap();
        assert!(matches!(
            verify(&env, &cfg, Scope::Home, &target, &fp),
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
            verify(&env, &cfg, Scope::Home, &ssh, &fp),
            Err(SafetyError::Protected(_))
        ));
        let out = p("/tmp/x");
        t.write("tmp/x", b"x");
        let fp = fingerprint(&env, &out).unwrap();
        assert!(matches!(
            verify(&env, &cfg, Scope::Home, &out, &fp),
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
        assert_eq!(verify(&env, &cfg, Scope::Home, &link, &fp), Ok(()));
        for s in [
            "/home/user/.config/foo/",
            "/home/user/.config/foo/.",
            "/home/user/.config//foo",
        ] {
            assert!(
                matches!(
                    verify(&env, &cfg, Scope::Home, &p(s), &fp),
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

    fn mkfifo(t: &FixtureTree, rel: &str) {
        use rustix::fs::{CWD, FileType, Mode, mknodat};
        mknodat(
            CWD,
            t.dir.path().join(rel),
            FileType::Fifo,
            Mode::from_raw_mode(0o600),
            0,
        )
        .unwrap();
    }

    #[test]
    fn user_temp_scope_rules() {
        if crate::testing::running_as_root() {
            eprintln!("skipped: temp cleanup is not offered as root");
            return;
        }
        let (t, env, cfg) = setup();
        t.write("tmp/x", b"x");
        t.write("tmp/d/y", b"y");
        t.write("home/user/.cache/z", b"z");
        t.symlink("tmp/link", "/tmp/x");
        mkfifo(&t, "tmp/fifo");
        let ut = Scope::UserTemp;
        // A build tree directly in /var/tmp follows the same rules.
        t.write("var/tmp/build/x", b"x");
        for s in ["/tmp/x", "/tmp/d", "/var/tmp/build"] {
            let target = p(s);
            assert!(is_allowed_target(&env, &cfg, ut, &target), "{s}");
            let fp = fingerprint(&env, &target).unwrap();
            assert_eq!(verify_now(&env, &cfg, ut, &target), Ok(fp), "{s}");
            assert_eq!(verify(&env, &cfg, ut, &target, &fp), Ok(()), "{s}");
            // Home rules don't reach /tmp.
            assert!(!is_allowed_target(&env, &cfg, Scope::Home, &target), "{s}");
        }
        for s in [
            "/tmp",
            "/tmp/d/y",
            "/home/user/.cache/z",
            "/var/tmp",
            "/var/tmp/build/x",
            "/var/x",
            "tmp/x",
        ] {
            assert!(!is_allowed_target(&env, &cfg, ut, &p(s)), "{s}");
        }
        assert!(matches!(
            verify_now(&env, &cfg, ut, &p("/tmp/d/y")),
            Err(SafetyError::OutsideAllowedRoots(_))
        ));
        assert!(matches!(
            verify_now(&env, &cfg, ut, &p("/home/user/.cache/z")),
            Err(SafetyError::OutsideAllowedRoots(_))
        ));
        // Symlinks and fifos are refused even with a matching fingerprint.
        for s in ["/tmp/link", "/tmp/fifo"] {
            let target = p(s);
            let fp = fingerprint(&env, &target).unwrap();
            assert!(
                matches!(
                    verify_now(&env, &cfg, ut, &target),
                    Err(SafetyError::Ineligible(..))
                ),
                "{s}"
            );
            assert!(
                matches!(
                    verify(&env, &cfg, ut, &target, &fp),
                    Err(SafetyError::Ineligible(..))
                ),
                "{s}"
            );
        }
        // Someone else's file.
        let mut other = env.clone();
        other.uid = env.uid.wrapping_add(1);
        let err = verify_now(&other, &cfg, ut, &p("/tmp/x")).unwrap_err();
        assert_eq!(err.to_string(), "refusing: /tmp/x is not owned by you");
        assert!(matches!(
            verify_now(&env, &cfg, ut, &p("/tmp/gone")),
            Err(SafetyError::Missing(_))
        ));
    }

    #[test]
    fn user_temp_refuses_symlinked_tmp() {
        let (t, env, cfg) = setup();
        t.write("elsewhere/x", b"x");
        t.symlink("tmp", "elsewhere");
        let target = p("/tmp/x");
        let fp = fingerprint(&env, &target).unwrap();
        assert_eq!(
            verify(&env, &cfg, Scope::UserTemp, &target, &fp),
            Err(SafetyError::SymlinkedParent(p("/tmp")))
        );
    }

    #[test]
    fn home_trash_scope_rules() {
        let (t, env, cfg) = setup();
        t.write("home/user/.local/share/Trash/files/x", b"x");
        t.write("home/user/.local/share/Trash/files/d/y", b"y");
        t.write("home/user/.local/share/Trash/info/x.trashinfo", b"i");
        t.write("home/user/.local/share/Trash/other", b"o");
        t.write("tmp/x", b"x");
        let ht = Scope::HomeTrash;
        let trash = "/home/user/.local/share/Trash";
        for s in [
            format!("{trash}/files/x"),
            format!("{trash}/files/d"),
            format!("{trash}/info/x.trashinfo"),
        ] {
            let target = p(&s);
            assert!(is_allowed_target(&env, &cfg, ht, &target), "{s}");
            let fp = fingerprint(&env, &target).unwrap();
            assert_eq!(verify_now(&env, &cfg, ht, &target), Ok(fp), "{s}");
            assert_eq!(verify(&env, &cfg, ht, &target, &fp), Ok(()), "{s}");
            assert!(
                !is_allowed_target(&env, &cfg, Scope::UserTemp, &target),
                "{s}"
            );
        }
        for s in [
            format!("{trash}/files/d/y"),
            trash.to_string(),
            format!("{trash}/files"),
            format!("{trash}/info"),
            format!("{trash}/other"),
            "/tmp/x".to_string(),
        ] {
            assert!(!is_allowed_target(&env, &cfg, ht, &p(&s)), "{s}");
            assert!(
                matches!(
                    verify_now(&env, &cfg, ht, &p(&s)),
                    Err(SafetyError::OutsideAllowedRoots(_) | SafetyError::Protected(_))
                ),
                "{s}"
            );
        }
    }

    #[test]
    fn home_trash_outside_home_refused() {
        let (t, mut env, cfg) = setup();
        t.write("data/Trash/files/x", b"x");
        t.write("data/Trash/info/x.trashinfo", b"i");
        env.data_home = t.dir.path().join("data");
        for s in ["/data/Trash/files/x", "/data/Trash/info/x.trashinfo"] {
            let target = p(s);
            assert!(!is_allowed_target(&env, &cfg, Scope::HomeTrash, &target));
            assert!(matches!(
                verify_now(&env, &cfg, Scope::HomeTrash, &target),
                Err(SafetyError::OutsideAllowedRoots(_))
            ));
        }
    }

    #[test]
    fn home_trash_requires_private_trash_dirs() {
        use std::os::unix::fs::PermissionsExt;
        let (t, env, cfg) = setup();
        t.write("home/user/.local/share/Trash/files/x", b"x");
        t.mkdir("home/user/.local/share/Trash/info");
        let target = p("/home/user/.local/share/Trash/files/x");
        let fp = fingerprint(&env, &target).unwrap();
        let ht = Scope::HomeTrash;
        assert_eq!(verify(&env, &cfg, ht, &target, &fp), Ok(()));
        let trash = t.dir.path().join("home/user/.local/share/Trash");
        std::fs::set_permissions(&trash, std::fs::Permissions::from_mode(0o777)).unwrap();
        let err = verify(&env, &cfg, ht, &target, &fp).unwrap_err();
        assert!(matches!(err, SafetyError::Ineligible(..)), "{err:?}");
        std::fs::set_permissions(&trash, std::fs::Permissions::from_mode(0o700)).unwrap();
        // Someone else's trash.
        let mut other = env.clone();
        other.uid = env.uid.wrapping_add(1);
        assert!(matches!(
            verify(&other, &cfg, ht, &target, &fp),
            Err(SafetyError::Ineligible(..))
        ));
        // A missing `info` dir fails closed too.
        std::fs::remove_dir(trash.join("info")).unwrap();
        assert!(verify(&env, &cfg, ht, &target, &fp).is_err());
        t.mkdir("home/user/.local/share/Trash/info");
        // `files` swapped for a symlink to an identical-looking tree.
        std::fs::rename(trash.join("files"), t.dir.path().join("real-files")).unwrap();
        t.symlink(
            "home/user/.local/share/Trash/files",
            "../../../../../real-files",
        );
        assert_eq!(
            verify(&env, &cfg, ht, &target, &fp),
            Err(SafetyError::SymlinkedParent(p(
                "~/.local/share/Trash/files"
            )))
        );
    }

    #[test]
    fn protect_applies_in_every_scope() {
        let (t, env, mut cfg) = setup();
        t.write("tmp/keep", b"k");
        t.write("tmp/excluded", b"e");
        t.write("home/user/.local/share/Trash/files/keep", b"k");
        t.mkdir("home/user/.local/share/Trash/info");
        t.write("home/user/.config/keep", b"k");
        cfg.protect.push(p("/tmp/keep"));
        cfg.protect
            .push(p("/home/user/.local/share/Trash/files/keep"));
        cfg.protect.push(p("/home/user/.config/keep"));
        cfg.exclude.push("/tmp/exclu*".into());
        for (scope, s) in [
            (Scope::UserTemp, "/tmp/keep"),
            (Scope::HomeTrash, "/home/user/.local/share/Trash/files/keep"),
            (Scope::Home, "/home/user/.config/keep"),
        ] {
            let target = p(s);
            assert!(is_protected(&env, &cfg, &target), "{s}");
            assert!(!is_allowed_target(&env, &cfg, scope, &target), "{s}");
            let fp = fingerprint(&env, &target).unwrap();
            assert!(
                matches!(
                    verify(&env, &cfg, scope, &target, &fp),
                    Err(SafetyError::Protected(_))
                ),
                "{s}"
            );
        }
        // Built-in protected paths too: `/` is never a temp child.
        for scope in [Scope::Home, Scope::UserTemp, Scope::HomeTrash] {
            assert!(!is_allowed_target(&env, &cfg, scope, &p("/")));
        }
        let excluded = p("/tmp/excluded");
        assert!(!is_allowed_target(&env, &cfg, Scope::UserTemp, &excluded));
        assert!(matches!(
            verify_now(&env, &cfg, Scope::UserTemp, &excluded),
            Err(SafetyError::OutsideAllowedRoots(_))
        ));
    }

    /// A fixture `/var/lib/systemd/coredump` with every directory from the
    /// root down set to 0755, so the root-owned chain check passes (the
    /// fixture user stands in for root, see `Env::system_owner_uid`).
    fn coredump_dir(t: &FixtureTree) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let dir = t.mkdir("var/lib/systemd/coredump");
        let mut cur = t.dir.path().to_path_buf();
        fs::set_permissions(&cur, fs::Permissions::from_mode(0o755)).unwrap();
        for c in ["var", "lib", "systemd", "coredump"] {
            cur.push(c);
            fs::set_permissions(&cur, fs::Permissions::from_mode(0o755)).unwrap();
        }
        dir
    }

    #[test]
    fn system_scope_allows_core_files_only() {
        let (t, env, cfg) = setup();
        coredump_dir(&t);
        t.write("var/lib/systemd/coredump/core.a.1", b"x");
        t.write("var/lib/systemd/coredump/other", b"x");
        t.write("var/lib/systemd/coredump/sub/core.deep", b"x");
        t.write("var/lib/systemd/coredump/core.dir/inner", b"x");
        t.write("var/lib/systemd/other/core.x", b"x");
        let sys = Scope::System;
        let good = p("/var/lib/systemd/coredump/core.a.1");
        assert!(is_allowed_target(&env, &cfg, sys, &good));
        let fp = verify_now(&env, &cfg, sys, &good).unwrap();
        verify(&env, &cfg, sys, &good, &fp).unwrap();
        for s in [
            "/var/lib/systemd/coredump/other",
            "/var/lib/systemd/coredump/core.",
            "/var/lib/systemd/coredump",
            "/var/lib/systemd/coredump/sub/core.deep",
            "/var/lib/systemd/other/core.x",
            "/tmp/core.x",
            "/var/lib/systemd/coredump/../coredump/core.a.1",
            "/",
        ] {
            assert!(!is_allowed_target(&env, &cfg, sys, &p(s)), "{s}");
        }
        // A directory named core.* is not a regular file.
        let dir = p("/var/lib/systemd/coredump/core.dir");
        assert!(is_allowed_target(&env, &cfg, sys, &dir));
        assert!(matches!(
            verify_now(&env, &cfg, sys, &dir),
            Err(SafetyError::Ineligible(_, _))
        ));
        // Protected and excluded paths stay refused.
        let mut cfg = cfg;
        cfg.exclude.push("/var/lib/systemd/coredump/core.a*".into());
        assert!(!is_allowed_target(&env, &cfg, sys, &good));
        let mut cfg2 = Config::default();
        cfg2.protect.push(good.clone());
        assert!(!is_allowed_target(&env, &cfg2, sys, &good));
        // Changed since scanned.
        let cfg3 = Config::default();
        t.write("var/lib/systemd/coredump/core.a.1", b"y");
        std::fs::remove_file(t.dir.path().join("var/lib/systemd/coredump/core.a.1")).unwrap();
        t.write("var/lib/systemd/coredump/core.a.1", b"z");
        assert!(matches!(
            verify(&env, &cfg3, sys, &good, &fp),
            Err(SafetyError::Changed(_))
        ));
    }

    #[test]
    fn system_scope_refuses_symlink_target() {
        let (t, env, cfg) = setup();
        coredump_dir(&t);
        t.write("var/lib/systemd/other", b"secret");
        t.symlink(
            "var/lib/systemd/coredump/core.link",
            "/var/lib/systemd/other",
        );
        let link = p("/var/lib/systemd/coredump/core.link");
        assert!(matches!(
            verify_now(&env, &cfg, Scope::System, &link),
            Err(SafetyError::Ineligible(_, _))
        ));
    }

    #[test]
    fn system_scope_refuses_symlinked_chain_dir() {
        let (t, env, cfg) = setup();
        let real = t.mkdir("elsewhere/coredump");
        t.mkdir("var/lib/systemd");
        coredump_dir(&t);
        fs::remove_dir(t.dir.path().join("var/lib/systemd/coredump")).unwrap();
        std::os::unix::fs::symlink(&real, t.dir.path().join("var/lib/systemd/coredump")).unwrap();
        fs::write(real.join("core.x"), b"x").unwrap();
        assert!(matches!(
            verify_now(
                &env,
                &cfg,
                Scope::System,
                &p("/var/lib/systemd/coredump/core.x")
            ),
            Err(SafetyError::SymlinkedParent(_))
        ));
    }

    #[test]
    fn system_scope_refuses_writable_parent() {
        use std::os::unix::fs::PermissionsExt;
        let (t, env, cfg) = setup();
        coredump_dir(&t);
        t.write("var/lib/systemd/coredump/core.x", b"x");
        let target = p("/var/lib/systemd/coredump/core.x");
        verify_now(&env, &cfg, Scope::System, &target).unwrap();
        // Any directory in the chain, the root included, group- or
        // other-writable refuses.
        for rel in [
            "",
            "var",
            "var/lib",
            "var/lib/systemd",
            "var/lib/systemd/coredump",
        ] {
            for bad in [0o775, 0o757] {
                let d = t.dir.path().join(rel);
                fs::set_permissions(&d, fs::Permissions::from_mode(bad)).unwrap();
                assert!(
                    matches!(
                        verify_now(&env, &cfg, Scope::System, &target),
                        Err(SafetyError::Ineligible(_, why)) if why.contains("writable")
                    ),
                    "{rel} {bad:o}"
                );
                fs::set_permissions(&d, fs::Permissions::from_mode(0o755)).unwrap();
            }
        }
        verify_now(&env, &cfg, Scope::System, &target).unwrap();
    }

    #[test]
    fn system_scope_refuses_non_root_parent() {
        let (t, mut env, cfg) = setup();
        coredump_dir(&t);
        t.write("var/lib/systemd/coredump/core.x", b"x");
        let target = p("/var/lib/systemd/coredump/core.x");
        verify_now(&env, &cfg, Scope::System, &target).unwrap();
        // The directories belong to the test user, not the expected owner.
        env.system_owner_uid = env.uid.wrapping_add(1);
        assert!(matches!(
            verify_now(&env, &cfg, Scope::System, &target),
            Err(SafetyError::Ineligible(_, why)) if why.contains("not owned by root")
        ));
    }

    /// `/`, `/etc` and `/opt` as 0755 directories, so the root-owned chain
    /// check passes (the fixture user stands in for root).
    fn system_tree_env(t: &FixtureTree) -> Env {
        use std::os::unix::fs::PermissionsExt;
        t.mkdir("etc");
        t.mkdir("opt");
        for rel in ["", "etc", "opt"] {
            fs::set_permissions(t.dir.path().join(rel), fs::Permissions::from_mode(0o755)).unwrap();
        }
        let mut env = t.env();
        env.set_system_owner_uid(env.uid);
        env
    }

    #[test]
    fn system_tree_allows_direct_children_only() {
        let (t, _env, cfg) = setup();
        let env = system_tree_env(&t);
        let st = Scope::SystemTree;
        for s in ["/opt/inky", "/etc/inky"] {
            assert!(is_allowed_target(&env, &cfg, st, &p(s)), "{s}");
        }
        for s in [
            "/opt/inky/sub",
            "/opt",
            "/etc",
            "/var/foo",
            "/etc/machine-id",
            "/etc/passwd-",
            "/etc/foo.pacsave",
            "/etc/foo.pacnew",
            "/etc/foo.bak",
            "/etc/foo.old",
            "/etc/.hidden",
            "/etc/../etc/inky",
        ] {
            assert!(!is_allowed_target(&env, &cfg, st, &p(s)), "{s}");
        }
        let mut cfg = cfg;
        cfg.exclude.push("/opt/inky".into());
        assert!(!is_allowed_target(&env, &cfg, st, &p("/opt/inky")));
    }

    #[test]
    fn system_leftover_names() {
        for n in ETC_NEVER {
            assert!(!is_system_leftover_name(OsStr::new(n)), "{n}");
        }
        for n in [
            "subuid",
            "subgid",
            "kernel",
            "cmdline.d",
            "secureboot",
            "crypttab.initramfs",
            "machine-info",
            "resolv.conf",
        ] {
            assert!(!is_system_leftover_name(OsStr::new(n)), "{n}");
        }
        // Unowned on stock Debian/Ubuntu and Fedora.
        for n in [
            "nsswitch.conf",
            "shells",
            "motd",
            "opt",
            "securetty",
            "dconf",
            "issue",
            "timezone",
            "login.defs",
            "apt",
            "dpkg",
            "dnf",
            "yum.repos.d",
            "rpm",
            "sysconfig",
            "alternatives",
            "selinux",
            "sudo.conf",
        ] {
            assert!(!is_system_leftover_name(OsStr::new(n)), "{n}");
        }
        assert!(!is_system_leftover_name(OsStr::new("")));
        assert!(is_system_leftover_name(OsStr::new("inky")));
        assert!(is_system_leftover_name(OsStr::new("inky.conf")));
    }

    #[test]
    fn system_tree_refuses_symlink_and_writable_parent() {
        use std::os::unix::fs::PermissionsExt;
        let (t, _env, cfg) = setup();
        let env = system_tree_env(&t);
        t.symlink("opt/x", "/etc");
        assert!(matches!(
            verify_now(&env, &cfg, Scope::SystemTree, &p("/opt/x")),
            Err(SafetyError::Ineligible(_, "is a symlink"))
        ));
        t.mkdir("opt/inky");
        t.write("opt/file", b"x");
        verify_now(&env, &cfg, Scope::SystemTree, &p("/opt/inky")).unwrap();
        verify_now(&env, &cfg, Scope::SystemTree, &p("/opt/file")).unwrap();
        fs::set_permissions(t.dir.path().join("opt"), fs::Permissions::from_mode(0o777)).unwrap();
        assert!(matches!(
            verify_now(&env, &cfg, Scope::SystemTree, &p("/opt/inky")),
            Err(SafetyError::Ineligible(_, "is writable by group or others"))
        ));
    }

    #[test]
    fn system_tree_refuses_special_files() {
        let (t, _env, cfg) = setup();
        let env = system_tree_env(&t);
        let fifo = t.dir.path().join("opt/fifo");
        rustix::fs::mknodat(
            rustix::fs::CWD,
            &fifo,
            rustix::fs::FileType::Fifo,
            rustix::fs::Mode::from_raw_mode(0o600),
            0,
        )
        .unwrap();
        assert!(matches!(
            verify_now(&env, &cfg, Scope::SystemTree, &p("/opt/fifo")),
            Err(SafetyError::Ineligible(_, why)) if why.contains("regular file or directory")
        ));
    }

    #[test]
    fn system_tree_verify_accepts_root_owned_chain() {
        let (t, _env, cfg) = setup();
        let env = system_tree_env(&t);
        t.mkdir("opt/inky");
        let target = p("/opt/inky");
        let st = Scope::SystemTree;
        let fp = verify_now(&env, &cfg, st, &target).unwrap();
        verify(&env, &cfg, st, &target, &fp).unwrap();
        // Keep the old directory alive so the new one has another inode.
        fs::rename(t.dir.path().join("opt/inky"), t.dir.path().join("opt/kept")).unwrap();
        t.mkdir("opt/inky");
        assert!(matches!(
            verify(&env, &cfg, st, &target, &fp),
            Err(SafetyError::Changed(_))
        ));
    }

    #[test]
    fn system_tree_serde_is_kebab_case() {
        assert_eq!(
            serde_json::to_string(&Scope::SystemTree).unwrap(),
            "\"system-tree\""
        );
    }

    #[test]
    fn system_scope_serde_is_kebab_case() {
        assert_eq!(serde_json::to_string(&Scope::System).unwrap(), "\"system\"");
    }

    #[test]
    fn scope_serde_is_kebab_case_and_defaults_to_home() {
        assert_eq!(Scope::default(), Scope::Home);
        for (scope, text) in [
            (Scope::Home, "\"home\""),
            (Scope::UserTemp, "\"user-temp\""),
            (Scope::HomeTrash, "\"home-trash\""),
        ] {
            assert_eq!(serde_json::to_string(&scope).unwrap(), text);
            assert_eq!(serde_json::from_str::<Scope>(text).unwrap(), scope);
        }
    }

    #[test]
    fn is_allowed_target_home_matches_user_target() {
        let (_t, env, cfg) = setup();
        for s in [
            "/home/user",
            "/home/user/.config/foo",
            "/tmp/x",
            "/home/user/.ssh/x",
        ] {
            assert_eq!(
                is_allowed_target(&env, &cfg, Scope::Home, &p(s)),
                is_allowed_user_target(&env, &cfg, &p(s)),
                "{s}"
            );
        }
    }

    #[test]
    fn fingerprint_detects_inode_reuse() {
        // Same dev, ino and kind but a different ctime: a file replaced and
        // handed the old inode.
        let t = FixtureTree::new();
        t.write("/home/user/.config/foo/bar", b"x");
        let env = t.env();
        let p = std::path::Path::new("/home/user/.config/foo/bar");
        let old = fingerprint(&env, p).unwrap();
        let mut stale = old;
        stale.ctime_nsec += 1;
        assert_ne!(old, stale);
        let cfg = Config::default();
        assert!(verify(&env, &cfg, Scope::Home, p, &old).is_ok());
        assert_eq!(
            verify(&env, &cfg, Scope::Home, p, &stale),
            Err(SafetyError::Changed("~/.config/foo/bar".into()))
        );
    }

    #[test]
    fn directory_gaining_an_entry_still_verifies() {
        let t = FixtureTree::new();
        t.write("/home/user/.config/foo/bar", b"x");
        let env = t.env();
        let d = std::path::Path::new("/home/user/.config/foo");
        let fp = fingerprint(&env, d).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        t.write("/home/user/.config/foo/new", b"y");
        assert!(verify(&env, &Config::default(), Scope::Home, d, &fp).is_ok());
    }
}
