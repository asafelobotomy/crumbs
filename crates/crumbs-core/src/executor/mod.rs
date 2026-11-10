//! Runs a [`Plan`]: every action is re-verified with `safety::verify`
//! immediately before it acts, on exactly the verified path, with no-follow
//! operations. Failures are recorded and the run continues.

pub(crate) mod files;
#[cfg(test)]
mod tests;

use std::cell::OnceCell;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::command::{CommandRunner, SystemRunner, USER_COMMAND_TIMEOUT, resolve_trusted};
use crate::config::Config;
use crate::env::{Env, display_path};
use crate::history::{self, HistoryAction, HistoryEntry};
use crate::model::{App, SourceKind};
use crate::mounts::mounts_at_or_below;
use crate::ownership::{OwnedIndex, TmpfilesDeclared, load_package_db, tmpfiles_declared};
use crate::plan::{Action, Plan};
use crate::privilege::Escalation;
use crate::privilege::batch::{
    PrivilegedRunner, Step, SystemPrivilegedRunner, build_script, parse_markers, sh_quote,
};
use crate::removal::lock_path;
use crate::safety::{self, Fingerprint, Scope};
use crate::size;
use crate::trash::{self, Trashed};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ActionResult {
    pub action: Action,
    pub ok: bool,
    pub error: Option<String>,
    pub history_id: String,
    /// A privileged step that never got administrator rights (none to
    /// ask for, or the prompt was dismissed). False for an action refused
    /// before the batch or one that ran and failed. Not part of the JSON.
    #[serde(skip)]
    pub rights_missing: bool,
    /// For a cleanup command that succeeded: the space its folders shrank
    /// by, measured before and after it ran (see [`measured_dirs`]). `None`
    /// when not measured; the plan's estimate stands then.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub freed: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ExecReport {
    pub batch: String,
    pub results: Vec<ActionResult>,
    pub freed: u64,
    pub escalation_failed: bool,
    /// The privileged batch ran but stopped before its last step (some
    /// markers came back, then none). Not part of the JSON: the results
    /// carry [`STOPPED_DURING`] and [`STOPPED_BEFORE`].
    #[serde(skip)]
    pub batch_stopped: bool,
    /// When the privileged batch gave back no marker at all (the escalation
    /// was denied or never started): what the escalation program wrote on
    /// stderr, if the runner captured it (see
    /// [`PrivilegedRun`](crate::privilege::batch::PrivilegedRun)). Not part
    /// of the JSON.
    #[serde(skip)]
    pub escalation_output: Vec<u8>,
}

/// What a successful action leaves for the history entry.
#[derive(Default)]
struct Done {
    trashed: Option<Trashed>,
    backup: Option<PathBuf>,
}

/// Why every privileged action failed when no escalation program exists.
pub const NO_ESCALATION: &str =
    "no way to get administrator rights (install systemd ≥ 256, polkit or sudo)";
/// Why a privileged step failed when no marker came back at all.
pub const ESCALATION_DENIED: &str = "privilege escalation was cancelled or denied";
/// Why the first step without a marker failed when earlier steps had
/// theirs: the root shell ended (was killed) while this step ran.
pub const STOPPED_DURING: &str =
    "the privileged run stopped during this step; it may have partly run";
/// Why a step after [`STOPPED_DURING`]'s failed.
pub const STOPPED_BEFORE: &str = "not run: the privileged run stopped";

/// Reports which of `names` are still installed from `source`, after a
/// `PackageRemove` claimed success.
pub type PostCheck<'a> = dyn Fn(&Env, SourceKind, &[String]) -> Result<Vec<String>, String> + 'a;

/// Runs every action (see [`execute_full`]) with the system runners. It
/// never escalates: every privileged action fails as if no escalation
/// program existed. The CLI's privilege check is the only way to root.
pub fn execute(env: &Env, cfg: &Config, plan: &Plan) -> ExecReport {
    execute_with(
        env,
        cfg,
        plan,
        &SystemRunner::with_timeout(USER_COMMAND_TIMEOUT),
    )
}

/// [`execute`] with the command runner injected; `UserCommand` actions run
/// through it. It never escalates (privileged actions fail with
/// [`NO_ESCALATION`]) and there is no package post-check: running anything
/// as root goes through [`execute_full`] or [`execute_many`] with an
/// escalation the caller chose.
pub fn execute_with(
    env: &Env,
    cfg: &Config,
    plan: &Plan,
    runner: &dyn CommandRunner,
) -> ExecReport {
    run_one(
        env,
        cfg,
        plan,
        runner,
        &SystemPrivilegedRunner::new(&env.root),
        None,
        None,
    )
}

/// Runs a plan.
///
/// First the privileged actions (`PrivilegedCommand`, `PackageRemove`, and
/// `Delete` in [`Scope::System`], which becomes `rm -f -- <path>`, and in
/// [`Scope::SystemTree`], which becomes a namespace-guarded
/// `rm -rf --one-file-system -- <path>`), in plan order, as one root script
/// run once through `priv_runner`: each program is resolved in the trusted
/// directories and each System or SystemTree `Delete` verified and
/// re-checked (as the user) right before the script is built; one that fails
/// is left out of it. No script runs when nothing is left, and a privileged
/// action that depends on another action is skipped (the batch runs before
/// everything else). A step's marker with status 0 is success; any other
/// status fails with the step's last output line. No marker at all means
/// the escalation was cancelled or denied; markers that stop part-way mean
/// the root shell was stopped ([`STOPPED_DURING`] for the step it was in,
/// [`STOPPED_BEFORE`] after it, and `batch_stopped`). Every `PackageRemove` that was
/// sent to the script is re-checked with `post_check`, whatever its step
/// said: it succeeds only if the step did and nothing is still installed,
/// its error lists what was removed and what is still installed, and its
/// history entry's `packages` holds only what is gone.
///
/// Then the remaining actions, in plan order. An action whose `depends_on`
/// action (an earlier one) did not succeed is skipped: `ok: false`, with
/// the reason, and recorded like any other.
///
/// Each attempt becomes one history entry, appended (and synced) right
/// after it ran (the privileged ones right after the batch, in plan
/// order), so an interrupted run still leaves a record of what it did.
/// When an entry can't be written, that result's error gets `"<append
/// error>; history not written"` (its `ok` still describes what happened);
/// other entries are unaffected. Results are in plan order.
///
/// `escalation_failed` is set when privileged steps were due but `esc` is
/// `None`, or no step's marker came back.
pub fn execute_full(
    env: &Env,
    cfg: &Config,
    plan: &Plan,
    runner: &dyn CommandRunner,
    priv_runner: &dyn PrivilegedRunner,
    esc: Option<&Escalation>,
    post_check: &PostCheck<'_>,
) -> ExecReport {
    run_one(env, cfg, plan, runner, priv_runner, esc, Some(post_check))
}

/// [`execute_full`] for several plans in one run: the privileged actions
/// of every plan (in plan order, then action order) go into ONE batch, so
/// the user is asked once. Then each plan's remaining actions run, plan
/// by plan. Each plan keeps its own batch id (consecutive, all newer than
/// the history) and its own `app_id` and `kind` in the history, and gets
/// its own report, in `plans` order. `escalation_failed` is set on the
/// reports of the plans whose privileged steps could not run.
pub fn execute_many(
    env: &Env,
    cfg: &Config,
    plans: &[Plan],
    runner: &dyn CommandRunner,
    priv_runner: &dyn PrivilegedRunner,
    esc: Option<&Escalation>,
    post_check: &PostCheck<'_>,
) -> Vec<ExecReport> {
    let refs: Vec<&Plan> = plans.iter().collect();
    run_plans(env, cfg, &refs, runner, priv_runner, esc, Some(post_check))
}

/// [`run_plans`] for one plan.
fn run_one(
    env: &Env,
    cfg: &Config,
    plan: &Plan,
    runner: &dyn CommandRunner,
    priv_runner: &dyn PrivilegedRunner,
    esc: Option<&Escalation>,
    post_check: Option<&PostCheck<'_>>,
) -> ExecReport {
    run_plans(env, cfg, &[plan], runner, priv_runner, esc, post_check)
        .pop()
        .unwrap_or_else(|| ExecReport {
            batch: history::next_batch_id(env),
            results: Vec::new(),
            freed: 0,
            escalation_failed: false,
            batch_stopped: false,
            escalation_output: Vec::new(),
        })
}

/// `n` batch ids, consecutive from [`history::next_batch_id`].
fn batch_ids(env: &Env, n: usize) -> Vec<String> {
    let first = history::next_batch_id(env);
    match first.parse::<u128>() {
        Ok(f) => (0..n)
            .map(|k| f.saturating_add(k as u128).to_string())
            .collect(),
        Err(_) => (0..n)
            .map(|k| {
                if k == 0 {
                    first.clone()
                } else {
                    format!("{first}-{k}")
                }
            })
            .collect(),
    }
}

/// Runs plans (see [`execute_many`]), with the post-check optional.
/// Without one, a `PackageRemove` whose step succeeded is trusted, and one
/// whose step failed claims nothing about what was removed.
fn run_plans(
    env: &Env,
    cfg: &Config,
    plans: &[&Plan],
    runner: &dyn CommandRunner,
    priv_runner: &dyn PrivilegedRunner,
    esc: Option<&Escalation>,
    post_check: Option<&PostCheck<'_>>,
) -> Vec<ExecReport> {
    let batches = batch_ids(env, plans.len());
    let checks = Rechecks::new(runner);
    // The batch's cleanup commands, measured before it runs. They all run
    // between the two measurements, so a folder two of them shrink cannot
    // be credited to either: those are not measured.
    let batch_dirs: Vec<PathBuf> = plans
        .iter()
        .flat_map(|p| &p.actions)
        .filter(|pa| matches!(pa.action, Action::PrivilegedCommand { .. }))
        .flat_map(|pa| measured_dirs(env, &pa.action))
        .collect();
    let shared = |dirs: &[PathBuf]| {
        dirs.iter()
            .any(|d| batch_dirs.iter().filter(|b| *b == d).count() > 1)
    };
    let before: Vec<Vec<Option<Snapshot>>> = plans
        .iter()
        .map(|p| {
            p.actions
                .iter()
                .map(|pa| match pa.action {
                    Action::PrivilegedCommand { .. } => {
                        let dirs = measured_dirs(env, &pa.action);
                        if shared(&dirs) {
                            None
                        } else {
                            measure(env, &dirs)
                        }
                    }
                    _ => None,
                })
                .collect()
        })
        .collect();
    let Privileged {
        mut attempts,
        failed,
        stopped,
        unmarked_stderr,
    } = run_privileged(env, cfg, &checks, plans, priv_runner, esc, post_check);
    for (k, plan_attempts) in attempts.iter_mut().enumerate() {
        for (i, attempt) in plan_attempts.iter_mut() {
            if attempt.outcome.is_ok() && before[k][*i].is_some() {
                let dirs = measured_dirs(env, &plans[k].actions[*i].action);
                attempt.freed = shrank(before[k][*i].as_ref(), measure(env, &dirs).as_ref());
            }
        }
    }
    let mut results: Vec<Vec<Option<ActionResult>>> =
        plans.iter().map(|p| vec![None; p.actions.len()]).collect();
    let mut freed = vec![0u64; plans.len()];
    // Every privileged entry is written right after the batch.
    for (k, plan_attempts) in attempts.into_iter().enumerate() {
        for (i, attempt) in plan_attempts {
            if attempt.outcome.is_ok() {
                let size = attempt.freed.unwrap_or(plans[k].actions[i].size);
                freed[k] = freed[k].saturating_add(size);
            }
            results[k][i] = Some(record(env, plans[k], &batches[k], i, attempt));
        }
    }
    let mut reports = Vec::with_capacity(plans.len());
    for (k, plan) in plans.iter().enumerate() {
        // Plans before `k` are finished; their results are what a
        // cross-plan dependency waits on.
        let (earlier, rest) = results.split_at_mut(k);
        let Some(results) = rest.first_mut() else {
            continue;
        };
        for (i, pa) in plan.actions.iter().enumerate() {
            if results[i].is_some() {
                continue;
            }
            let mut attempt = Attempt::new(action_target(&pa.action));
            let skip = skip_reason(env, plan, results, i, pa.depends_on)
                .or_else(|| cross_skip_reason(env, plans, earlier, pa.depends_on_plan));
            attempt.outcome = match skip {
                Some(reason) => Err(reason),
                // A command has no path to verify; its program is resolved in
                // the trusted directories right now instead.
                None if matches!(pa.action, Action::UserCommand { .. }) => {
                    let dirs = measured_dirs(env, &pa.action);
                    let before = measure(env, &dirs);
                    let ran = run_command(
                        env,
                        &pa.action,
                        runner,
                        &mut attempt.path,
                        &mut attempt.argv,
                    );
                    if ran.is_ok() && before.is_some() {
                        attempt.freed = shrank(before.as_ref(), measure(env, &dirs).as_ref());
                    }
                    ran
                }
                None => verify_path(env, cfg, &checks, pa, &attempt.path)
                    .and_then(|()| act(env, &pa.action, &pa.fingerprint, &batches[k])),
            };
            if attempt.outcome.is_ok() {
                freed[k] = freed[k].saturating_add(attempt.freed.unwrap_or(pa.size));
            }
            results[i] = Some(record(env, plan, &batches[k], i, attempt));
        }
        reports.push(ExecReport {
            batch: batches[k].clone(),
            results: results.iter().flatten().cloned().collect(),
            freed: freed[k],
            escalation_failed: failed[k],
            batch_stopped: stopped[k],
            escalation_output: if failed[k] {
                unmarked_stderr.clone()
            } else {
                Vec::new()
            },
        });
    }
    reports
}

/// One action's attempt, as it goes into its history entry.
struct Attempt {
    /// In-system target: the path, or the program for a command.
    path: PathBuf,
    argv: Option<Vec<String>>,
    packages: Option<Vec<(String, String)>>,
    outcome: Result<Done, String>,
    /// See [`ActionResult::rights_missing`].
    rights_missing: bool,
    /// See [`ActionResult::freed`].
    freed: Option<u64>,
}

impl Attempt {
    fn new(path: PathBuf) -> Self {
        Attempt {
            path,
            argv: None,
            packages: None,
            outcome: Ok(Done::default()),
            rights_missing: false,
            freed: None,
        }
    }
}

/// The in-system folders a known cleanup command shrinks, so the executor
/// can measure what it freed: the package caches (`paccache`, `pacman
/// -Sc`, `apt-get clean`, `dnf clean`), the journal (`journalctl
/// --vacuum-*`), `systemd-tmpfiles --clean --prefix=...`, `flatpak
/// uninstall --unused` and the `containers` commands of podman and Docker.
/// Empty for any other command.
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
        // What `containers` runs: rootless podman keeps everything here.
        "podman" if (has("image") && has("prune")) || (has("volume") && has("rm")) => {
            vec![env.unsys(&env.data_home).join("containers/storage")]
        }
        "docker" if (has("image") && has("prune")) || (has("volume") && has("rm")) => {
            vec!["/var/lib/docker".into()]
        }
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
type Snapshot = Vec<(bool, u64, u64)>;

/// Measures `dirs`; `None` when there are none, or when one exists but
/// cannot be listed at all (Docker's root-only `/var/lib/docker`): an
/// empty measurement before and after would claim nothing was freed.
fn measure(env: &Env, dirs: &[PathBuf]) -> Option<Snapshot> {
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
fn shrank(before: Option<&Snapshot>, after: Option<&Snapshot>) -> Option<u64> {
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
fn is_privileged(pa: &crate::plan::PlannedAction) -> bool {
    match pa.action {
        Action::PrivilegedCommand { .. } | Action::PackageRemove { .. } => true,
        Action::Delete { .. } => matches!(pa.scope, Scope::System | Scope::SystemTree),
        Action::Trash { .. } | Action::EditFile { .. } | Action::UserCommand { .. } => false,
    }
}

/// `safety::verify` plus the scope's own re-checks, right before acting.
fn verify_path(
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
struct Rechecks<'a> {
    /// Runs the package database's own queries (rpm).
    runner: &'a dyn CommandRunner,
    /// The owned index, or `None` when it could not be read completely.
    owned: OnceCell<Option<OwnedIndex>>,
    /// The `tmpfiles.d` declarations, or `None` when they could not be read.
    declared: OnceCell<Option<TmpfilesDeclared>>,
}

#[cfg(test)]
thread_local! {
    /// How many times this thread read the pacman database for a re-check.
    pub(crate) static PACMAN_LOADS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

impl<'a> Rechecks<'a> {
    fn new(runner: &'a dyn CommandRunner) -> Self {
        Rechecks {
            runner,
            owned: OnceCell::new(),
            declared: OnceCell::new(),
        }
    }

    fn owned(&self, env: &Env) -> Option<&OwnedIndex> {
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

    fn declared(&self, env: &Env) -> Option<&TmpfilesDeclared> {
        self.declared
            .get_or_init(|| tmpfiles_declared(env).ok())
            .as_ref()
    }
}

/// Why a [`Scope::SystemTree`] target is refused right before its step.
pub const TREE_LOCKED: &str = "pacman is busy (database lock present)";
pub const TREE_MOUNT_BELOW: &str = "a mount point is below it";
pub const TREE_PM_RUNNING: &str = "a package manager is running";

/// Process names (`/proc/<pid>/comm`, cut to 15 bytes by the kernel) of
/// package managers. dpkg and rpm hold fcntl locks on files that always
/// exist, so unlike pacman's `db.lck` their lock cannot be seen from a file;
/// a running one is seen from its process instead.
const PACKAGE_MANAGERS: [&str; 13] = [
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
fn package_manager_running(env: &Env) -> bool {
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
fn recheck_system_tree(env: &Env, checks: &Rechecks, path: &Path) -> Result<OsString, String> {
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

/// Per plan, its privileged attempts with their action indices.
type PlanAttempts = Vec<Vec<(usize, Attempt)>>;

/// What [`run_privileged`] did, per plan.
struct Privileged {
    attempts: PlanAttempts,
    /// Its escalation failed.
    failed: Vec<bool>,
    /// The batch stopped part-way through its steps.
    stopped: Vec<bool>,
    /// The escalation's captured stderr when the batch gave back no
    /// marker at all; empty otherwise.
    unmarked_stderr: Vec<u8>,
}

/// Runs the privileged actions of every plan as one batch; returns, per
/// plan, their attempts in plan order (with action indices), whether its
/// escalation failed, and whether the batch stopped part-way through its
/// steps; and the escalation's stderr when no marker came back.
///
/// No marker at all means the escalation was cancelled or denied: every
/// step is not run, for lack of rights. When some markers came back and
/// then none, the root shell ran and was stopped: the first step without a
/// marker gets [`STOPPED_DURING`], the later ones [`STOPPED_BEFORE`]; that
/// is not an escalation failure, and no rights were missing.
fn run_privileged(
    env: &Env,
    cfg: &Config,
    checks: &Rechecks,
    plans: &[&Plan],
    priv_runner: &dyn PrivilegedRunner,
    esc: Option<&Escalation>,
    post_check: Option<&PostCheck<'_>>,
) -> Privileged {
    let mut attempts: PlanAttempts = plans.iter().map(|_| Vec::new()).collect();
    let mut failed = vec![false; plans.len()];
    let mut stopped = vec![false; plans.len()];
    let mut steps: Vec<Step> = Vec::new();
    // (plan, index into `attempts[plan]`) of the actions that became
    // `steps`, in order.
    let mut stepped: Vec<(usize, usize)> = Vec::new();
    for (k, plan) in plans.iter().enumerate() {
        for (i, pa) in plan.actions.iter().enumerate() {
            if !is_privileged(pa) {
                continue;
            }
            let mut attempt = Attempt::new(action_target(&pa.action));
            let step = match (pa.depends_on, pa.depends_on_plan) {
                (Some(dep), _) => Err(format!(
                    "skipped: privileged actions run first and cannot wait for {}",
                    shown_action(env, plan, dep)
                )),
                (None, Some((ep, ea))) => Err(format!(
                    "skipped: privileged actions run first and cannot wait for {}",
                    plans
                        .get(ep)
                        .map_or_else(|| format!("action {ea}"), |p| shown_action(env, p, ea))
                )),
                (None, None) => privileged_step(env, cfg, checks, pa, &mut attempt),
            };
            match step {
                Ok(step) => {
                    // Fails closed: only a marker read below can change this.
                    attempt.outcome = Err(ESCALATION_DENIED.into());
                    stepped.push((k, attempts[k].len()));
                    steps.push(step);
                }
                Err(e) => attempt.outcome = Err(e),
            }
            attempts[k].push((i, attempt));
        }
    }
    let done = |attempts, failed, stopped, unmarked_stderr| Privileged {
        attempts,
        failed,
        stopped,
        unmarked_stderr,
    };
    if steps.is_empty() {
        return done(attempts, failed, stopped, Vec::new());
    }
    let fail_all = |attempts: &mut Vec<Vec<(usize, Attempt)>>, msg: &str| {
        for &(k, j) in &stepped {
            attempts[k][j].1.outcome = Err(msg.to_string());
        }
    };
    let mark_missing = |attempts: &mut Vec<Vec<(usize, Attempt)>>| {
        for &(k, j) in &stepped {
            attempts[k][j].1.rights_missing = true;
        }
    };
    let Some(esc) = esc else {
        fail_all(&mut attempts, NO_ESCALATION);
        mark_missing(&mut attempts);
        for &(k, _) in &stepped {
            failed[k] = true;
        }
        return done(attempts, failed, stopped, Vec::new());
    };
    let script = match build_script(&steps) {
        Ok(s) => s,
        Err(e) => {
            fail_all(&mut attempts, &e.to_string());
            return done(attempts, failed, stopped, Vec::new());
        }
    };
    let mut unmarked_stderr = Vec::new();
    let ends: Vec<StepEnd> = match priv_runner.run_captured(esc, &script) {
        Ok(run) => {
            let outcomes = parse_markers(&run.stdout, steps.len());
            // Markers come back in order, so this is how many steps ended.
            let ended = outcomes.iter().take_while(|o| o.status.is_some()).count();
            if ended == 0 {
                unmarked_stderr = run.stderr;
            }
            (0..steps.len())
                .map(|n| match outcomes.get(n).map(|o| (o.status, o)) {
                    Some((Some(0), _)) => StepEnd::Succeeded,
                    Some((Some(status), o)) => StepEnd::Exited(status, last_line(&o.output)),
                    Some((None, _)) | None if ended == 0 => {
                        StepEnd::NotRun(ESCALATION_DENIED.into())
                    }
                    Some((None, _)) | None if n == ended => StepEnd::Stopped(STOPPED_DURING),
                    Some((None, _)) | None => StepEnd::Stopped(STOPPED_BEFORE),
                })
                .collect()
        }
        Err(e) => {
            let msg = format!("privilege escalation failed: {e}");
            vec![StepEnd::NotRun(msg); steps.len()]
        }
    };
    for (&(k, j), end) in stepped.iter().zip(ends) {
        let (i, attempt) = &mut attempts[k][j];
        match end {
            StepEnd::NotRun(_) => {
                failed[k] = true;
                attempt.rights_missing = true;
            }
            StepEnd::Stopped(_) => stopped[k] = true,
            StepEnd::Succeeded | StepEnd::Exited(..) => {}
        }
        let action = &plans[k].actions[*i].action;
        attempt.outcome = match action {
            Action::PackageRemove { .. } => check_packages(env, action, end, post_check, attempt),
            _ => match end {
                StepEnd::Succeeded => Ok(Done::default()),
                StepEnd::Exited(status, line) => {
                    Err(line.unwrap_or_else(|| format!("exited with status {status}")))
                }
                StepEnd::NotRun(msg) => Err(msg),
                StepEnd::Stopped(msg) => Err(msg.into()),
            },
        };
    }
    done(attempts, failed, stopped, unmarked_stderr)
}

/// How a batch step ended.
#[derive(Clone)]
enum StepEnd {
    Succeeded,
    /// A non-zero status, with the step's last output line.
    Exited(i32, Option<String>),
    /// No marker came back (nor any other); why.
    NotRun(String),
    /// No marker came back after earlier steps' did: the batch stopped.
    Stopped(&'static str),
}

/// A `PackageRemove`'s outcome once its step ran (or should have): the
/// packages `post_check` no longer finds are recorded as removed, and the
/// action succeeds only if the step did and none is left.
fn check_packages(
    env: &Env,
    action: &Action,
    end: StepEnd,
    post_check: Option<&PostCheck<'_>>,
    attempt: &mut Attempt,
) -> Result<Done, String> {
    let Action::PackageRemove {
        source,
        names,
        versions,
        program,
        ..
    } = action
    else {
        return Err("not a package removal".into());
    };
    let Some(post_check) = post_check else {
        // Nothing to ask: only the step's own status is known.
        return match end {
            StepEnd::Succeeded => {
                attempt.packages = Some(
                    names
                        .iter()
                        .enumerate()
                        .map(|(k, n)| (n.clone(), versions.get(k).cloned().unwrap_or_default()))
                        .collect(),
                );
                Ok(Done::default())
            }
            StepEnd::Exited(status, line) => Err(exit_message(program, status, line)),
            StepEnd::NotRun(msg) => Err(msg),
            StepEnd::Stopped(msg) => Err(msg.into()),
        };
    };
    let still = match post_check(env, *source, names) {
        Ok(still) => still,
        Err(reason) => {
            // Nothing is known about what was removed: claim nothing.
            let mut parts = Vec::new();
            match end {
                StepEnd::Succeeded => {}
                StepEnd::Exited(status, line) => parts.push(exit_message(program, status, line)),
                StepEnd::NotRun(msg) => parts.push(msg),
                StepEnd::Stopped(msg) => parts.push(msg.into()),
            }
            parts.push(format!("could not verify removal: {reason}"));
            return Err(parts.join("; "));
        }
    };
    let removed: Vec<(String, String)> = names
        .iter()
        .enumerate()
        .filter(|(_, n)| !still.contains(n))
        .map(|(k, n)| (n.clone(), versions.get(k).cloned().unwrap_or_default()))
        .collect();
    let mut parts: Vec<String> = Vec::new();
    match end {
        StepEnd::Succeeded => {}
        StepEnd::Exited(status, line) => parts.push(exit_message(program, status, line)),
        StepEnd::NotRun(msg) => parts.push(msg),
        StepEnd::Stopped(msg) => parts.push(msg.into()),
    }
    let ok = parts.is_empty() && still.is_empty();
    if !ok && !removed.is_empty() {
        let names: Vec<&str> = removed.iter().map(|(n, _)| n.as_str()).collect();
        parts.push(format!("removed: {}", names.join(", ")));
    }
    if !still.is_empty() {
        parts.push(format!("still installed: {}", still.join(", ")));
    }
    attempt.packages = Some(removed);
    if ok {
        Ok(Done::default())
    } else {
        Err(parts.join("; "))
    }
}

/// The batch step for a privileged action, filling in its history fields.
/// A System or SystemTree `Delete` is verified now; a program not
/// installed fails.
///
/// A SystemTree `Delete` becomes `sh -c <tree_script> crumbs-rm <user mount
/// namespace> <path>`, which runs `rm -rf --one-file-system -- <path>` only
/// while pacman holds no lock and only in the user's mount namespace; a
/// missing trusted `readlink` refuses it ([`TREE_NS_UNREADABLE`]).
fn privileged_step(
    env: &Env,
    cfg: &Config,
    checks: &Rechecks,
    pa: &crate::plan::PlannedAction,
    attempt: &mut Attempt,
) -> Result<Step, String> {
    let resolve = |program: &str| {
        resolve_trusted(env, program).ok_or_else(|| format!("{program} is not installed"))
    };
    let (program, args) = match &pa.action {
        Action::Delete { path } if pa.scope == Scope::SystemTree => {
            safety::verify(env, cfg, pa.scope, path, &pa.fingerprint).map_err(|e| e.to_string())?;
            let ns = recheck_system_tree(env, checks, path)?;
            // Without it the guard cannot compare namespaces.
            let readlink =
                resolve_trusted(env, "readlink").ok_or_else(|| TREE_NS_UNREADABLE.to_string())?;
            let sh = resolve("sh")?;
            let rm = resolve("rm")?;
            let args = vec![
                OsString::from("-c"),
                tree_script(&lock_path(env), &readlink, &rm),
                OsString::from("crumbs-rm"),
                ns,
                env.sys_path(path).into_os_string(),
            ];
            return Ok(Step { program: sh, args });
        }
        Action::Delete { path } => {
            verify_path(env, cfg, checks, pa, path)?;
            let rm = resolve("rm")?;
            let args = vec![
                OsString::from("-f"),
                OsString::from("--"),
                env.sys_path(path).into_os_string(),
            ];
            return Ok(Step { program: rm, args });
        }
        Action::PrivilegedCommand { program, args } => (program, args),
        Action::PackageRemove { program, args, .. } => (program, args),
        Action::Trash { .. } | Action::EditFile { .. } | Action::UserCommand { .. } => {
            return Err("not a privileged action".into());
        }
    };
    attempt.argv = Some(args.clone());
    let resolved = resolve(program)?;
    attempt.path = env.unsys(&resolved);
    Ok(Step {
        program: resolved,
        args: args.iter().map(OsString::from).collect(),
    })
}

/// `<program> exited with status N[: <last output line>]`.
fn exit_message(program: &str, status: i32, line: Option<String>) -> String {
    match line {
        Some(line) => format!("{program} exited with status {status}: {line}"),
        None => format!("{program} exited with status {status}"),
    }
}

/// The last non-empty line of a command's output, trimmed.
fn last_line(output: &str) -> Option<String> {
    output
        .lines()
        .rev()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .map(str::to_string)
}

/// Writes the attempt's history entry and returns its result.
fn record(env: &Env, plan: &Plan, batch: &str, i: usize, attempt: Attempt) -> ActionResult {
    let pa = &plan.actions[i];
    let id = format!("{batch}-{i}");
    let (ok, error, done) = match attempt.outcome {
        Ok(done) => (true, None, done),
        Err(e) => (false, Some(e), Done::default()),
    };
    let entry = HistoryEntry {
        id: id.clone(),
        batch: batch.to_string(),
        time: history::now_rfc3339(),
        action: match pa.action {
            Action::Trash { .. } => HistoryAction::Trash,
            Action::Delete { .. } => HistoryAction::Delete,
            Action::EditFile { .. } => HistoryAction::EditFile,
            Action::UserCommand { .. } => HistoryAction::Command,
            Action::PrivilegedCommand { .. } => HistoryAction::PrivilegedCommand,
            Action::PackageRemove { .. } => HistoryAction::PackageRemove,
        },
        path: attempt.path,
        trashed: done.trashed,
        backup: done.backup,
        size: attempt.freed.unwrap_or(pa.size),
        ok,
        error: error.clone(),
        app_id: plan.app_id.clone(),
        restores: None,
        argv: attempt.argv,
        packages: attempt.packages,
        source: match &pa.action {
            Action::PackageRemove { source, .. } => Some(*source),
            _ => None,
        },
        kind: plan.kind.clone(),
        scope: match pa.action {
            // Only root deletes say where they ran (docs/json-schema.md).
            Action::Delete { .. } if matches!(pa.scope, Scope::System | Scope::SystemTree) => {
                Some(pa.scope)
            }
            _ => None,
        },
    };
    let error = match history::append(env, std::slice::from_ref(&entry)) {
        Ok(()) => error,
        Err(e) => {
            let suffix = format!("{e}; history not written");
            Some(match error {
                Some(prev) => format!("{prev}; {suffix}"),
                None => suffix,
            })
        }
    };
    ActionResult {
        action: pa.action.clone(),
        ok,
        error,
        history_id: id,
        rights_missing: attempt.rights_missing,
        freed: if ok { attempt.freed } else { None },
    }
}

/// What stands for the action in results and history: its path, or for a
/// command its program name.
fn action_target(action: &Action) -> PathBuf {
    match action {
        Action::UserCommand { program, .. }
        | Action::PrivilegedCommand { program, .. }
        | Action::PackageRemove { program, .. } => PathBuf::from(program),
        other => other.path().map(Path::to_path_buf).unwrap_or_default(),
    }
}

fn human_duration(d: std::time::Duration) -> String {
    match d.as_secs() {
        s if s >= 60 && s % 60 == 0 => format!("{} min", s / 60),
        s => format!("{s} s"),
    }
}

/// Runs a `UserCommand`. On return `path` is the in-system program path (or
/// the bare name when it is not installed) and `argv` its arguments.
fn run_command(
    env: &Env,
    action: &Action,
    runner: &dyn CommandRunner,
    path: &mut PathBuf,
    argv: &mut Option<Vec<String>>,
) -> Result<Done, String> {
    let Action::UserCommand { program, args } = action else {
        return Err("not a command".into());
    };
    *argv = Some(args.clone());
    let resolved =
        resolve_trusted(env, program).ok_or_else(|| format!("{program} is not installed"))?;
    *path = env.unsys(&resolved);
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let out = runner.run(&resolved, &args).map_err(|e| e.to_string())?;
    if out.timed_out {
        return Err(format!(
            "{program} timed out after {}",
            human_duration(USER_COMMAND_TIMEOUT)
        ));
    }
    if out.status == 0 {
        return Ok(Done::default());
    }
    Err(last_line(&out.stderr).unwrap_or_else(|| format!("exited with status {}", out.status)))
}

/// A `UserTemp` target that is a package manager's build tree rather
/// than a temporary file: a direct child of `/var/tmp` (only `aur-build`
/// plans those). In `/tmp`, even a `pamac-build-*` entry keeps temp's
/// staleness re-check.
fn is_build_tree(path: &Path) -> bool {
    path.parent() == Some(Path::new("/var/tmp"))
}

/// Scope rules beyond `verify` that are re-run right before acting: a
/// `UserTemp` entry must still be stale (old, unused, nothing open); a
/// `SystemTree` target must pass [`recheck_system_tree`].
fn recheck_scope(
    env: &Env,
    cfg: &Config,
    checks: &Rechecks,
    scope: Scope,
    path: &Path,
) -> Result<(), String> {
    match scope {
        // A build tree (`aur-build`): kept while a process works in it,
        // runs a program from it or has a file open in it.
        Scope::UserTemp if is_build_tree(path) => {
            match process_working_in(env, path).or_else(|| process_using(env, path)) {
                Some((pid, comm)) => Err(format!(
                    "refusing: {} is in use by {comm} (pid {pid})",
                    display_path(env, path)
                )),
                None => Ok(()),
            }
        }
        Scope::UserTemp => crate::bloat::temp::still_stale(env, cfg, path, env.now())
            .map_err(|reason| format!("refusing: {} {reason}", display_path(env, path))),
        // Unreachable in practice: a SystemTree `Delete` is privileged and
        // re-checked in `privileged_step`, and no other action has this
        // scope. Kept so the rule holds whoever calls this.
        Scope::SystemTree => recheck_system_tree(env, checks, path).map(drop),
        Scope::Home | Scope::HomeTrash | Scope::System => Ok(()),
    }
}

/// Why an action must not run: the earlier action it depends on failed or
/// was itself skipped. A dependency on itself or a later action never
/// succeeds.
fn skip_reason(
    env: &Env,
    plan: &Plan,
    results: &[Option<ActionResult>],
    index: usize,
    depends_on: Option<usize>,
) -> Option<String> {
    let dep = depends_on?;
    if dep < index
        && results
            .get(dep)
            .is_some_and(|r| r.as_ref().is_some_and(|r| r.ok))
    {
        return None;
    }
    Some(format!(
        "skipped: {} was not removed",
        shown_action(env, plan, dep)
    ))
}

/// Why an action must be skipped because the action of an earlier plan that
/// covered its dependency (`(plan, action)`) did not succeed; `None` when it
/// did, or there is no such dependency.
fn cross_skip_reason(
    env: &Env,
    plans: &[&Plan],
    earlier: &[Vec<Option<ActionResult>>],
    depends_on_plan: Option<(usize, usize)>,
) -> Option<String> {
    let (ep, ea) = depends_on_plan?;
    let ok = earlier
        .get(ep)
        .and_then(|r| r.get(ea))
        .is_some_and(|r| r.as_ref().is_some_and(|r| r.ok));
    if ok {
        return None;
    }
    let shown = plans
        .get(ep)
        .map_or_else(|| format!("action {ea}"), |p| shown_action(env, p, ea));
    Some(format!(
        "skipped: {shown} was not removed (it covered this action)"
    ))
}

/// The action at `index` as shown in a skip reason.
fn shown_action(env: &Env, plan: &Plan, index: usize) -> String {
    plan.actions.get(index).map_or_else(
        || format!("action {index}"),
        |d| display_path(env, &action_target(&d.action)),
    )
}

fn io_msg(env: &Env, in_system: &Path, e: &io::Error) -> String {
    format!("{}: {e}", display_path(env, in_system))
}

/// Acts on an already-verified path.
fn act(env: &Env, action: &Action, fp: &Fingerprint, batch: &str) -> Result<Done, String> {
    match action {
        Action::Trash { path } => trash::move_to_trash(env, path)
            .map(|t| Done {
                trashed: Some(t),
                backup: None,
            })
            .map_err(|e| match e {
                crate::error::CoreError::Io { source, .. } => io_msg(env, path, &source),
                other => other.to_string(),
            }),
        Action::Delete { path } => delete(env, path).map(|()| Done::default()),
        Action::EditFile { path, remove_lines } => edit_file(env, path, remove_lines, fp, batch)
            .map(|backup| Done {
                trashed: None,
                backup,
            }),
        Action::UserCommand { .. }
        | Action::PrivilegedCommand { .. }
        | Action::PackageRemove { .. } => Err("a command cannot be run as a path action".into()),
    }
}

/// Removes the entry itself: a symlink or file is unlinked, a real
/// directory is removed recursively (std's `remove_dir_all` does not follow
/// symlinks inside the tree). A directory holding a mount point (another
/// filesystem, or a bind mount of the same one) is refused untouched.
fn delete(env: &Env, path: &Path) -> Result<(), String> {
    delete_with(env, path, &size::mount_id)
}

/// [`delete`] with the mount lookup injected (tests fake a mount point).
fn delete_with(
    env: &Env,
    path: &Path,
    mount_of: &dyn Fn(&Path, &fs::Metadata) -> Option<u64>,
) -> Result<(), String> {
    let real = env.sys_path(path);
    let meta = fs::symlink_metadata(&real).map_err(|e| io_msg(env, path, &e))?;
    if !meta.file_type().is_dir() {
        return fs::remove_file(&real).map_err(|e| io_msg(env, path, &e));
    }
    // `remove_dir_all` would empty a filesystem mounted inside the tree.
    match size::crosses_mount(&real, mount_of) {
        Ok(false) => {}
        Ok(true) => {
            return Err(format!(
                "refusing: {} contains another filesystem",
                display_path(env, path)
            ));
        }
        Err(e) => {
            return Err(format!(
                "refusing: cannot check {} for other filesystems: {e}",
                display_path(env, path)
            ));
        }
    }
    let mut removed = fs::remove_dir_all(&real);
    if removed
        .as_ref()
        .is_err_and(|e| e.kind() == std::io::ErrorKind::PermissionDenied)
    {
        // Read-only folders (Go's module cache): make the ones you own
        // writable again, as `go clean -modcache` does, and try once more.
        make_owned_dirs_writable(&real, env.uid);
        removed = fs::remove_dir_all(&real);
    }
    removed.map_err(|e| {
        let msg = io_msg(env, path, &e);
        // Part of the tree may already be gone; say so.
        if fs::symlink_metadata(&real).is_ok() {
            format!("partially deleted: {msg}")
        } else {
            msg
        }
    })
}

/// Gives every real directory in the tree at `real` that `uid` owns its
/// owner's read, write and search bits, so what is in it can be removed.
/// Symlinks are not followed, and nothing is made writable for anyone else.
fn make_owned_dirs_writable(real: &Path, uid: u32) {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let mut stack = vec![real.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(meta) = fs::symlink_metadata(&dir) else {
            continue;
        };
        if !meta.file_type().is_dir() || meta.uid() != uid {
            continue;
        }
        let mode = meta.permissions().mode();
        if mode & 0o700 != 0o700 {
            let _ = fs::set_permissions(&dir, fs::Permissions::from_mode(mode | 0o700));
        }
        if let Ok(rd) = fs::read_dir(&dir) {
            stack.extend(rd.flatten().map(|e| e.path()));
        }
    }
}

/// Filters `path` in place. Returns the in-system backup path, or `None`
/// when no line matched (nothing is written then).
fn edit_file(
    env: &Env,
    path: &Path,
    remove_lines: &[String],
    fp: &Fingerprint,
    batch: &str,
) -> Result<Option<PathBuf>, String> {
    let real = env.sys_path(path);
    let err = |e: io::Error| io_msg(env, path, &e);
    if fs::symlink_metadata(&real)
        .map_err(err)?
        .file_type()
        .is_symlink()
    {
        return Err(format!(
            "refusing: {} is a symlink; edit it by hand",
            display_path(env, path)
        ));
    }
    // Still opened no-follow in case it is swapped after the check.
    let file = files::open_regular_nofollow(&real).map_err(err)?;
    let meta = file.metadata().map_err(err)?;
    if meta.dev() != fp.dev || meta.ino() != fp.ino {
        return Err(format!(
            "refusing: {} changed since it was scanned",
            display_path(env, path)
        ));
    }
    let original = files::read_all(&file).map_err(err)?;
    let filtered = files::filter_lines(&original, remove_lines);
    if filtered == original {
        return Ok(None);
    }
    let mode = meta.mode() & 0o7777;
    let backup = files::with_suffix(&real, &format!(".crumbs-bak-{batch}"));
    let backup_in_system = env.unsys(&backup);
    files::write_new(&backup, &original, mode).map_err(|e| io_msg(env, &backup_in_system, &e))?;
    if let Err(e) = files::replace_via_temp(&real, &filtered, mode, &format!(".crumbs-tmp-{batch}"))
    {
        let _ = fs::remove_file(&backup);
        return Err(err(e));
    }
    Ok(Some(backup_in_system))
}

/// Names too common to say which app a process belongs to: never matched
/// by basename (exe, comm or argv0).
const GENERIC_BASENAMES: [&str; 15] = [
    "sh", "bash", "dash", "zsh", "fish", "env", "python", "python3", "perl", "ruby", "node",
    "electron", "java", "flatpak", "bwrap",
];

/// Whether `n` is a [`GENERIC_BASENAMES`] name or `python3.<digits>`.
fn is_generic(n: &OsStr) -> bool {
    GENERIC_BASENAMES.iter().any(|g| OsStr::new(g) == n)
        || n.as_bytes()
            .strip_prefix(b"python3.")
            .is_some_and(|v| !v.is_empty() && v.iter().all(u8::is_ascii_digit))
}

/// The kernel cuts `comm` at 15 bytes.
const COMM_LEN: usize = 15;

/// A running process that runs a program from `in_system` (its executable
/// is there) or has a file open there, as `(pid, command name)`; the
/// lowest pid when there are several.
pub fn process_using(env: &Env, in_system: &Path) -> Option<(u32, String)> {
    let real = env.sys_path(in_system);
    let inside = |p: &Path| p.starts_with(in_system) || p.starts_with(&real);
    let rd = fs::read_dir(env.sys_path(Path::new("/proc"))).ok()?;
    rd.flatten()
        .filter_map(|e| {
            let pid: u32 = e.file_name().to_str()?.parse().ok()?;
            let dir = e.path();
            let exe = fs::read_link(dir.join("exe")).is_ok_and(|t| inside(&strip_deleted(&t)));
            let open = exe
                || fs::read_dir(dir.join("fd")).is_ok_and(|fds| {
                    fds.flatten()
                        .any(|f| fs::read_link(f.path()).is_ok_and(|t| inside(&t)))
                });
            if !open {
                return None;
            }
            let comm = fs::read_to_string(dir.join("comm")).unwrap_or_default();
            let comm = comm.trim();
            let comm = if comm.is_empty() { "a process" } else { comm };
            Some((pid, comm.to_string()))
        })
        .min()
}

/// A running process whose current directory is `in_system` or below it
/// (a build or a shell working there), as `(pid, command name)`; the
/// lowest pid when there are several.
pub fn process_working_in(env: &Env, in_system: &Path) -> Option<(u32, String)> {
    let real = env.sys_path(in_system);
    let rd = fs::read_dir(env.sys_path(Path::new("/proc"))).ok()?;
    rd.flatten()
        .filter_map(|e| {
            let pid: u32 = e.file_name().to_str()?.parse().ok()?;
            let cwd = fs::read_link(e.path().join("cwd")).ok()?;
            if !(cwd.starts_with(in_system) || cwd.starts_with(&real)) {
                return None;
            }
            let comm = fs::read_to_string(e.path().join("comm")).unwrap_or_default();
            let comm = comm.trim();
            let comm = if comm.is_empty() { "a process" } else { comm };
            Some((pid, comm.to_string()))
        })
        .min()
}

/// PIDs (sorted) of processes that look like `app`: the basename of the
/// `exe` link (without the kernel's ` (deleted)` suffix), of `comm` or of
/// `cmdline`'s first argument is one of `exec_names`; `cmdline`'s first
/// or second argument is the app's `program` (or `program_link`) path; the `exe` link is the
/// root-joined `location`; or `environ` holds `APPIMAGE=<location>`. Names
/// in [`GENERIC_BASENAMES`] never match by basename. Unreadable entries
/// are skipped.
pub fn running_processes(env: &Env, app: &App) -> Vec<u32> {
    let Ok(dir) = fs::read_dir(env.sys_path(Path::new("/proc"))) else {
        return Vec::new();
    };
    let location_real = app.location.as_deref().map(|l| env.sys_path(l));
    let appimage_var = app.location.as_deref().map(|l| {
        let mut v = b"APPIMAGE=".to_vec();
        v.extend_from_slice(l.as_os_str().as_bytes());
        v
    });
    let exec_names: Vec<&OsStr> = app
        .exec_names
        .iter()
        .filter_map(|e| Path::new(e).file_name())
        .filter(|n| !is_generic(n))
        .collect();
    let programs: Vec<&Path> = [app.program.as_deref(), app.program_link.as_deref()]
        .into_iter()
        .flatten()
        .collect();
    let named = |n: &OsStr| exec_names.contains(&n);
    let comm_named = |comm: &[u8]| {
        exec_names.iter().any(|n| {
            let n = n.as_bytes();
            n == comm || (n.len() > COMM_LEN && n[..COMM_LEN] == *comm)
        })
    };
    let mut pids: Vec<u32> = dir
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name();
            let name = name.to_str()?;
            if name.is_empty() || !name.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            let pid: u32 = name.parse().ok()?;
            let dir = entry.path();
            let by_exe = fs::read_link(dir.join("exe")).is_ok_and(|target| {
                let target = strip_deleted(&target);
                target.file_name().is_some_and(named)
                    || location_real.as_deref() == Some(target.as_path())
            });
            let by_comm = || {
                fs::read(dir.join("comm")).is_ok_and(|c| {
                    let c = c.strip_suffix(b"\n").unwrap_or(&c);
                    !c.is_empty() && comm_named(c)
                })
            };
            let by_argv0 = || {
                fs::read(dir.join("cmdline")).is_ok_and(|c| {
                    let argv0 = c.split(|&b| b == 0).next().unwrap_or_default();
                    Path::new(OsStr::from_bytes(argv0))
                        .file_name()
                        .is_some_and(named)
                })
            };
            let by_script = || {
                fs::read(dir.join("cmdline")).is_ok_and(|c| {
                    // A launcher script or interpreter names the program as
                    // argv0 or argv1; only its exact path counts, never the
                    // interpreter's name.
                    c.split(|&b| b == 0)
                        .take(2)
                        .any(|arg| programs.iter().any(|p| p.as_os_str().as_bytes() == arg))
                })
            };
            let by_env = || {
                appimage_var.as_deref().is_some_and(|var| {
                    fs::read(dir.join("environ"))
                        .is_ok_and(|bytes| bytes.split(|&b| b == 0).any(|kv| kv == var))
                })
            };
            (by_exe || by_comm() || by_argv0() || by_script() || by_env()).then_some(pid)
        })
        .collect();
    pids.sort_unstable();
    pids
}

/// `target` without the ` (deleted)` the kernel appends to the `exe` link of
/// a binary that was unlinked or replaced.
fn strip_deleted(target: &Path) -> PathBuf {
    target
        .as_os_str()
        .as_bytes()
        .strip_suffix(b" (deleted)")
        .map_or_else(
            || target.to_path_buf(),
            |t| PathBuf::from(OsStr::from_bytes(t)),
        )
}
