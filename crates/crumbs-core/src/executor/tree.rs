//! Re-checks and the guarded removal script for whole system trees.

use super::*;

/// Why a [`Scope::SystemTree`] target is refused right before its step.
pub const TREE_LOCKED: &str = "pacman is busy (database lock present)";
pub const TREE_MOUNT_BELOW: &str = "a mount point is below it";
pub const TREE_PM_RUNNING: &str = "a package manager is running";

/// Process names (`/proc/<pid>/comm`, cut to 15 bytes by the kernel) of
/// package managers. dpkg and rpm hold fcntl locks on files that always
/// exist, so unlike pacman's `db.lck` their lock cannot be seen from a file;
/// a running one is seen from its process instead.
pub(super) const PACKAGE_MANAGERS: [&str; 13] = [
    "pacman",
    "dpkg",
    "apt",
    "apt-get",
    "aptitude",
    "unattended-upgr",
    "dnf",
    "dnf5",
    "dnf-automatic",
    "yum",
    "rpm",
    "zypper",
    "packagekitd",
];

/// Whether any process under `/proc` is a package manager.
pub(super) fn package_manager_running(env: &Env) -> bool {
    let Ok(dir) = fs::read_dir(env.sys_path(Path::new("/proc"))) else {
        return false;
    };
    dir.flatten().any(|e| {
        fs::read_to_string(e.path().join("comm"))
            .is_ok_and(|c| PACKAGE_MANAGERS.contains(&c.trim_end()))
    })
}
pub const TREE_MOUNTS_UNREADABLE: &str = "the mount table could not be read";
pub const TREE_DB_UNREADABLE: &str = "the package database could not be read";
pub const TREE_OWNED: &str = "a package owns files in it now";
pub const TREE_NS_UNREADABLE: &str = "the mount namespace could not be read";

/// The exit status of a tree step whose guard refused: pacman's lock is
/// present, or the root shell is in another mount namespace than crumbs
/// (the mount check above would not hold there).
pub const GUARD_REFUSED_STATUS: i32 = 97;

/// The re-checks for a [`Scope::SystemTree`] target, after
/// `safety::verify`: pacman holds no lock, no mount point lies at or
/// below it, no package owns anything at or below it (the database read
/// once per run, and fully), `tmpfiles.d` declares nothing it covers (read
/// once per run; unreadable fails as [`TREE_DB_UNREADABLE`]), and this
/// process's mount namespace can be read. Returns that namespace, which
/// the root step must match.
pub(super) fn recheck_system_tree(
    env: &Env,
    checks: &Rechecks,
    path: &Path,
) -> Result<OsString, String> {
    match fs::symlink_metadata(lock_path(env)) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        _ => return Err(TREE_LOCKED.into()),
    }
    if package_manager_running(env) {
        return Err(TREE_PM_RUNNING.into());
    }
    match mounts_at_or_below(env, path) {
        Ok(found) if found.is_empty() => {}
        Ok(_) => return Err(TREE_MOUNT_BELOW.into()),
        Err(_) => return Err(TREE_MOUNTS_UNREADABLE.into()),
    }
    let owned = checks.owned(env).ok_or(TREE_DB_UNREADABLE)?;
    if owned.owns_at_or_below(path) {
        return Err(TREE_OWNED.into());
    }
    let declared = checks.declared(env).ok_or(TREE_DB_UNREADABLE)?;
    if declared.covers(path) {
        return Err(TREE_OWNED.into());
    }
    env.user_mount_ns()
        .ok_or_else(|| TREE_NS_UNREADABLE.to_string())
}

/// The `sh -c` script of a tree step. It deletes `$2` only when pacman's
/// database lock `lock` (root-joined, so a fixture stays inside its root)
/// is absent (no file and no symlink, dangling or not) and the root shell's
/// mount namespace is `$1` (the user's), so the user-side checks describe
/// what root sees; otherwise it exits [`GUARD_REFUSED_STATUS`]. `readlink` and `rm` are trusted program paths.
pub(crate) fn tree_script(lock: &Path, readlink: &Path, rm: &Path) -> OsString {
    let lock = sh_quote(lock.as_os_str());
    // A dangling symlink at the lock counts as a lock too.
    let mut script = b"{ [ -e ".to_vec();
    script.extend_from_slice(&lock);
    script.extend_from_slice(b" ] || [ -L ");
    script.extend_from_slice(&lock);
    script.extend_from_slice(
        format!(
            " ]; }} && {{ echo \"pacman is busy (database lock present); not deleting\" >&2; \
             exit {GUARD_REFUSED_STATUS}; }}; test \"$("
        )
        .as_bytes(),
    );
    script.extend(sh_quote(readlink.as_os_str()));
    script.extend_from_slice(
        format!(
            " /proc/self/ns/mnt)\" = \"$1\" || {{ echo \"mount namespace differs; not \
             deleting\" >&2; exit {GUARD_REFUSED_STATUS}; }}; exec "
        )
        .as_bytes(),
    );
    script.extend(sh_quote(rm.as_os_str()));
    script.extend_from_slice(b" -rf --one-file-system -- \"$2\"");
    OsString::from_vec(script)
}
