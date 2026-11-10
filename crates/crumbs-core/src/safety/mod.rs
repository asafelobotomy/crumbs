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

mod verify;
pub use verify::*;
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

#[cfg(test)]
mod tests;
