//! What an action shrinks, and the re-checks run right before each step.

use super::*;

/// The in-system folders a known cleanup command shrinks, so the executor
/// can measure what it freed: the package caches (`paccache`, `pacman
/// -Sc`, `apt-get clean`, `dnf clean`), the journal (`journalctl
/// --vacuum-*`), `systemd-tmpfiles --clean --prefix=...`, `flatpak
/// uninstall --unused`. Empty for any other command (container storage is
/// not walked: it is large, and the images' own sizes say what goes).
pub fn measured_dirs(env: &Env, action: &Action) -> Vec<PathBuf> {
    let (Action::UserCommand { program, args } | Action::PrivilegedCommand { program, args }) =
        action
    else {
        return Vec::new();
    };
    let base = Path::new(program)
        .file_name()
        .and_then(OsStr::to_str)
        .unwrap_or_default();
    let has = |a: &str| args.iter().any(|x| x == a);
    let dirs: Vec<PathBuf> = match base {
        "paccache" => vec!["/var/cache/pacman/pkg".into()],
        "pacman" if has("-Sc") || has("-Scc") => vec!["/var/cache/pacman/pkg".into()],
        "journalctl" if args.iter().any(|a| a.starts_with("--vacuum-")) => {
            vec!["/var/log/journal".into()]
        }
        "apt-get" if has("clean") || has("autoclean") => vec!["/var/cache/apt/archives".into()],
        "dnf" if has("clean") => vec!["/var/cache/dnf".into(), "/var/cache/libdnf5".into()],
        "systemd-tmpfiles" if has("--clean") => args
            .iter()
            .filter_map(|a| a.strip_prefix("--prefix="))
            .map(PathBuf::from)
            .collect(),
        "flatpak" if has("uninstall") && has("--unused") => {
            if has("--user") {
                vec![env.unsys(&env.data_home).join("flatpak")]
            } else {
                vec!["/var/lib/flatpak".into()]
            }
        }
        _ => Vec::new(),
    };
    dirs.into_iter().filter(|d| d.is_absolute()).collect()
}

/// One measurement of a command's folders: for each, whether it exists,
/// the bytes read and how many parts could not be read.
pub(super) type Snapshot = Vec<(bool, u64, u64)>;

/// Measures `dirs`; `None` when there are none, or when one exists but
/// cannot be listed at all (Docker's root-only `/var/lib/docker`): an
/// empty measurement before and after would claim nothing was freed.
pub(super) fn measure(env: &Env, dirs: &[PathBuf]) -> Option<Snapshot> {
    if dirs.is_empty() {
        return None;
    }
    for d in dirs {
        let real = env.sys_path(d);
        if fs::symlink_metadata(&real).is_ok() && fs::read_dir(&real).is_err() {
            return None;
        }
    }
    let exists: Vec<bool> = dirs
        .iter()
        .map(|d| fs::symlink_metadata(env.sys_path(d)).is_ok())
        .collect();
    Some(
        size::disk_usage_many(env, dirs)
            .into_iter()
            .zip(exists)
            .map(|(u, there)| {
                if there {
                    (true, u.bytes, u.errors)
                } else {
                    (false, 0, 0)
                }
            })
            .collect(),
    )
}

/// What a command freed: the drop between two measurements, counted only
/// when they are comparable. Each folder must exist both times or neither
/// (a missing one is empty), and its unreadable parts (root-only download
/// folders, other users' private /tmp folders) must be as many after as
/// before: what could not be read is left out of both, so the result is at
/// most what was freed. `None` when not comparable or nothing existed.
pub(super) fn shrank(before: Option<&Snapshot>, after: Option<&Snapshot>) -> Option<u64> {
    let (before, after) = (before?, after?);
    if before.len() != after.len() || !before.iter().any(|(there, ..)| *there) {
        return None;
    }
    let mut total = 0u64;
    for ((b_there, b_bytes, b_err), (a_there, a_bytes, a_err)) in before.iter().zip(after) {
        if b_there != a_there || b_err != a_err {
            return None;
        }
        total = total.saturating_add(b_bytes.saturating_sub(*a_bytes));
    }
    Some(total)
}

/// Whether the action goes into the privileged batch.
pub(super) fn is_privileged(pa: &crate::plan::PlannedAction) -> bool {
    match pa.action {
        Action::PrivilegedCommand { .. } | Action::PackageRemove { .. } => true,
        Action::Delete { .. } => matches!(pa.scope, Scope::System | Scope::SystemTree),
        Action::Trash { .. } | Action::EditFile { .. } | Action::UserCommand { .. } => false,
    }
}

/// `safety::verify` plus the scope's own re-checks, right before acting.
pub(super) fn verify_path(
    env: &Env,
    cfg: &Config,
    checks: &Rechecks,
    pa: &crate::plan::PlannedAction,
    path: &Path,
) -> Result<(), String> {
    safety::verify(env, cfg, pa.scope, path, &pa.fingerprint)
        .map_err(|e| e.to_string())
        .and_then(|()| recheck_scope(env, cfg, checks, pa.scope, path))
}

/// State the re-checks share across one run: the package database and
/// `tmpfiles.d` are each read at most once, however many targets need them.
pub(super) struct Rechecks<'a> {
    /// Runs the package database's own queries (rpm).
    pub(super) runner: &'a dyn CommandRunner,
    /// The owned index, or `None` when it could not be read completely.
    pub(super) owned: OnceCell<Option<OwnedIndex>>,
    /// The `tmpfiles.d` declarations, or `None` when they could not be read.
    pub(super) declared: OnceCell<Option<TmpfilesDeclared>>,
}

#[cfg(test)]
thread_local! {
    /// How many times this thread read the pacman database for a re-check.
    pub(crate) static PACMAN_LOADS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

impl<'a> Rechecks<'a> {
    pub(super) fn new(runner: &'a dyn CommandRunner) -> Self {
        Rechecks {
            runner,
            owned: OnceCell::new(),
            declared: OnceCell::new(),
        }
    }

    pub(super) fn owned(&self, env: &Env) -> Option<&OwnedIndex> {
        self.owned
            .get_or_init(|| {
                #[cfg(test)]
                PACMAN_LOADS.with(|n| n.set(n.get() + 1));
                match load_package_db(env, self.runner) {
                    Ok(Some(idx)) if idx.is_complete() => Some(idx),
                    _ => None,
                }
            })
            .as_ref()
    }

    pub(super) fn declared(&self, env: &Env) -> Option<&TmpfilesDeclared> {
        self.declared
            .get_or_init(|| tmpfiles_declared(env).ok())
            .as_ref()
    }
}
