//! Fingerprints and the checks that re-verify a target right before it is touched.

use super::*;

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

pub(super) fn real_path(env: &Env, in_system: &Path) -> PathBuf {
    match in_system.strip_prefix("/") {
        Ok(rest) => env.root.join(rest),
        Err(_) => env.root.join(in_system),
    }
}

pub(super) fn lstat(env: &Env, in_system: &Path) -> io::Result<Fingerprint> {
    fs::symlink_metadata(real_path(env, in_system)).map(|m| fingerprint_of(&m))
}

pub(super) fn fingerprint_of(m: &fs::Metadata) -> Fingerprint {
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
pub(super) fn check_location(
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
pub(super) fn check_system_chain(env: &Env, parent: &Path, shown: &str) -> Result<(), SafetyError> {
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
pub(super) fn check_parent_chain(
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
pub(super) fn check_real_dir(env: &Env, dir: &Path, shown: &str) -> Result<(), SafetyError> {
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
