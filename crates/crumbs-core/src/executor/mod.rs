//! Runs a [`Plan`]: every action is re-verified with `safety::verify`
//! immediately before it acts, on exactly the verified path, with no-follow
//! operations. Failures are recorded and the run continues.

pub(crate) mod files;
#[cfg(test)]
mod tests;

use std::ffi::{OsStr, OsString};
use std::fs;
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::command::{CommandRunner, SystemRunner, USER_COMMAND_TIMEOUT, resolve_trusted};
use crate::config::Config;
use crate::env::{Env, display_path};
use crate::history::{self, HistoryAction, HistoryEntry};
use crate::model::{App, SourceKind};
use crate::plan::{Action, Plan};
use crate::privilege::batch::{
    PrivilegedRunner, Step, SystemPrivilegedRunner, build_script, parse_markers,
};
use crate::privilege::{Escalation, detect_escalation};
use crate::safety::{self, Fingerprint, Scope};
use crate::size;
use crate::trash::{self, Trashed};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ActionResult {
    pub action: Action,
    pub ok: bool,
    pub error: Option<String>,
    pub history_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ExecReport {
    pub batch: String,
    pub results: Vec<ActionResult>,
    pub freed: u64,
    pub escalation_failed: bool,
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
/// Why a privileged step failed when its marker never came back.
pub const ESCALATION_DENIED: &str = "privilege escalation was cancelled or denied";

/// Reports which of `names` are still installed from `source`, after a
/// `PackageRemove` claimed success.
pub type PostCheck<'a> = dyn Fn(&Env, SourceKind, &[String]) -> Vec<String> + 'a;

/// Runs every action (see [`execute_full`]) with the system runners and
/// the escalation program [`detect_escalation`] finds.
pub fn execute(env: &Env, cfg: &Config, plan: &Plan) -> ExecReport {
    execute_with(
        env,
        cfg,
        plan,
        &SystemRunner::with_timeout(USER_COMMAND_TIMEOUT),
    )
}

/// [`execute`] with the command runner injected; `UserCommand` actions run
/// through it. The escalation program is detected (only when the plan holds
/// privileged actions) with the query runner, and never under a fake root:
/// there the programs are user-writable fixtures, so every privileged
/// action fails as if no escalation program existed. There is no package
/// post-check.
pub fn execute_with(
    env: &Env,
    cfg: &Config,
    plan: &Plan,
    runner: &dyn CommandRunner,
) -> ExecReport {
    let esc = (env.root == Path::new("/") && plan.actions.iter().any(is_privileged))
        .then(|| detect_escalation(env, &SystemRunner::new()))
        .flatten();
    execute_full(
        env,
        cfg,
        plan,
        runner,
        &SystemPrivilegedRunner,
        esc.as_ref(),
        &|_, _, _| Vec::new(),
    )
}

/// Runs a plan.
///
/// First the privileged actions (`PrivilegedCommand`, `PackageRemove`, and
/// `Delete` in [`Scope::System`], which becomes `rm -f -- <path>`), in plan
/// order, as one root script run once through `priv_runner`: each program
/// is resolved in the trusted directories and each System `Delete`
/// verified (as the user) right before the script is built; one that fails
/// is left out of it. No script runs when nothing is left, and a privileged
/// action that depends on another action is skipped (the batch runs before
/// everything else). A step's marker with status 0 is success; any other
/// status fails with the step's last output line; a missing marker means
/// the escalation was cancelled or denied. Every `PackageRemove` that was
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
/// `None`, or a step's marker never came back.
pub fn execute_full(
    env: &Env,
    cfg: &Config,
    plan: &Plan,
    runner: &dyn CommandRunner,
    priv_runner: &dyn PrivilegedRunner,
    esc: Option<&Escalation>,
    post_check: &PostCheck<'_>,
) -> ExecReport {
    let batch = history::next_batch_id(env);
    let mut results: Vec<Option<ActionResult>> = vec![None; plan.actions.len()];
    let mut freed = 0u64;
    let (attempts, escalation_failed) =
        run_privileged(env, cfg, plan, priv_runner, esc, post_check);
    for (i, attempt) in attempts {
        let pa = &plan.actions[i];
        if attempt.outcome.is_ok() {
            freed = freed.saturating_add(pa.size);
        }
        results[i] = Some(record(env, plan, &batch, i, attempt));
    }
    for (i, pa) in plan.actions.iter().enumerate() {
        if results[i].is_some() {
            continue;
        }
        let mut attempt = Attempt::new(action_target(&pa.action));
        attempt.outcome = match skip_reason(env, plan, &results, i, pa.depends_on) {
            Some(reason) => Err(reason),
            // A command has no path to verify; its program is resolved in
            // the trusted directories right now instead.
            None if matches!(pa.action, Action::UserCommand { .. }) => run_command(
                env,
                &pa.action,
                runner,
                &mut attempt.path,
                &mut attempt.argv,
            ),
            None => verify_path(env, cfg, pa, &attempt.path)
                .and_then(|()| act(env, &pa.action, &pa.fingerprint, &batch)),
        };
        if attempt.outcome.is_ok() {
            freed = freed.saturating_add(pa.size);
        }
        results[i] = Some(record(env, plan, &batch, i, attempt));
    }
    ExecReport {
        batch,
        results: results.into_iter().flatten().collect(),
        freed,
        escalation_failed,
    }
}

/// One action's attempt, as it goes into its history entry.
struct Attempt {
    /// In-system target: the path, or the program for a command.
    path: PathBuf,
    argv: Option<Vec<String>>,
    packages: Option<Vec<(String, String)>>,
    outcome: Result<Done, String>,
}

impl Attempt {
    fn new(path: PathBuf) -> Self {
        Attempt {
            path,
            argv: None,
            packages: None,
            outcome: Ok(Done::default()),
        }
    }
}

/// Whether the action goes into the privileged batch.
fn is_privileged(pa: &crate::plan::PlannedAction) -> bool {
    match pa.action {
        Action::PrivilegedCommand { .. } | Action::PackageRemove { .. } => true,
        Action::Delete { .. } => pa.scope == Scope::System,
        Action::Trash { .. } | Action::EditFile { .. } | Action::UserCommand { .. } => false,
    }
}

/// `safety::verify` plus the scope's own re-checks, right before acting.
fn verify_path(
    env: &Env,
    cfg: &Config,
    pa: &crate::plan::PlannedAction,
    path: &Path,
) -> Result<(), String> {
    safety::verify(env, cfg, pa.scope, path, &pa.fingerprint)
        .map_err(|e| e.to_string())
        .and_then(|()| recheck_scope(env, cfg, pa.scope, path))
}

/// Runs the privileged actions as one batch; returns their attempts in plan
/// order (with plan indices) and whether escalation failed.
fn run_privileged(
    env: &Env,
    cfg: &Config,
    plan: &Plan,
    priv_runner: &dyn PrivilegedRunner,
    esc: Option<&Escalation>,
    post_check: &PostCheck<'_>,
) -> (Vec<(usize, Attempt)>, bool) {
    let mut attempts: Vec<(usize, Attempt)> = Vec::new();
    let mut steps: Vec<Step> = Vec::new();
    // Indices into `attempts` of the actions that became `steps`, in order.
    let mut stepped: Vec<usize> = Vec::new();
    for (i, pa) in plan.actions.iter().enumerate() {
        if !is_privileged(pa) {
            continue;
        }
        let mut attempt = Attempt::new(action_target(&pa.action));
        let step = match pa.depends_on {
            Some(dep) => Err(format!(
                "skipped: privileged actions run first and cannot wait for {}",
                shown_action(env, plan, dep)
            )),
            None => privileged_step(env, cfg, pa, &mut attempt),
        };
        match step {
            Ok(step) => {
                // Fails closed: only a marker read below can change this.
                attempt.outcome = Err(ESCALATION_DENIED.into());
                stepped.push(attempts.len());
                steps.push(step);
            }
            Err(e) => attempt.outcome = Err(e),
        }
        attempts.push((i, attempt));
    }
    if steps.is_empty() {
        return (attempts, false);
    }
    let mut fail_all = |msg: &str| {
        for &k in &stepped {
            attempts[k].1.outcome = Err(msg.to_string());
        }
    };
    let Some(esc) = esc else {
        fail_all(NO_ESCALATION);
        return (attempts, true);
    };
    let script = match build_script(&steps) {
        Ok(s) => s,
        Err(e) => {
            fail_all(&e.to_string());
            return (attempts, false);
        }
    };
    let (ends, escalation_failed): (Vec<StepEnd>, bool) = match priv_runner.run(esc, &script) {
        Ok(stdout) => {
            let outcomes = parse_markers(&stdout, steps.len());
            let ends: Vec<StepEnd> = (0..steps.len())
                .map(|n| match outcomes.get(n).map(|o| (o.status, o)) {
                    Some((Some(0), _)) => StepEnd::Succeeded,
                    Some((Some(status), o)) => StepEnd::Exited(status, last_line(&o.output)),
                    Some((None, _)) | None => StepEnd::NotRun(ESCALATION_DENIED.into()),
                })
                .collect();
            let missing = ends.iter().any(|e| matches!(e, StepEnd::NotRun(_)));
            (ends, missing)
        }
        Err(e) => {
            let msg = format!("privilege escalation failed: {e}");
            (vec![StepEnd::NotRun(msg); steps.len()], true)
        }
    };
    for (&k, end) in stepped.iter().zip(ends) {
        let (i, attempt) = &mut attempts[k];
        let action = &plan.actions[*i].action;
        attempt.outcome = match action {
            Action::PackageRemove { .. } => check_packages(env, action, end, post_check, attempt),
            _ => match end {
                StepEnd::Succeeded => Ok(Done::default()),
                StepEnd::Exited(status, line) => {
                    Err(line.unwrap_or_else(|| format!("exited with status {status}")))
                }
                StepEnd::NotRun(msg) => Err(msg),
            },
        };
    }
    (attempts, escalation_failed)
}

/// How a batch step ended.
#[derive(Clone)]
enum StepEnd {
    Succeeded,
    /// A non-zero status, with the step's last output line.
    Exited(i32, Option<String>),
    /// No marker came back; why.
    NotRun(String),
}

/// A `PackageRemove`'s outcome once its step ran (or should have): the
/// packages `post_check` no longer finds are recorded as removed, and the
/// action succeeds only if the step did and none is left.
fn check_packages(
    env: &Env,
    action: &Action,
    end: StepEnd,
    post_check: &PostCheck<'_>,
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
    let still = post_check(env, *source, names);
    let removed: Vec<(String, String)> = names
        .iter()
        .enumerate()
        .filter(|(_, n)| !still.contains(n))
        .map(|(k, n)| (n.clone(), versions.get(k).cloned().unwrap_or_default()))
        .collect();
    let mut parts: Vec<String> = Vec::new();
    match end {
        StepEnd::Succeeded => {}
        StepEnd::Exited(status, line) => parts.push(match line {
            Some(line) => format!("{program} exited with status {status}: {line}"),
            None => format!("{program} exited with status {status}"),
        }),
        StepEnd::NotRun(msg) => parts.push(msg),
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
/// A System `Delete` is verified now; a program not installed fails.
fn privileged_step(
    env: &Env,
    cfg: &Config,
    pa: &crate::plan::PlannedAction,
    attempt: &mut Attempt,
) -> Result<Step, String> {
    let resolve = |program: &str| {
        resolve_trusted(env, program).ok_or_else(|| format!("{program} is not installed"))
    };
    let (program, args) = match &pa.action {
        Action::Delete { path } => {
            verify_path(env, cfg, pa, path)?;
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
        size: pa.size,
        ok,
        error: error.clone(),
        app_id: plan.app_id.clone(),
        restores: None,
        argv: attempt.argv,
        packages: attempt.packages,
        kind: plan.kind.clone(),
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

/// Scope rules beyond `verify` that are re-run right before acting: a
/// `UserTemp` entry must still be stale (old, unused, nothing open).
fn recheck_scope(env: &Env, cfg: &Config, scope: Scope, path: &Path) -> Result<(), String> {
    if scope != Scope::UserTemp {
        return Ok(());
    }
    crate::bloat::temp::still_stale(env, cfg, path, env.now())
        .map_err(|reason| format!("refusing: {} {reason}", display_path(env, path)))
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
    fs::remove_dir_all(&real).map_err(|e| {
        let msg = io_msg(env, path, &e);
        // Part of the tree may already be gone; say so.
        if fs::symlink_metadata(&real).is_ok() {
            format!("partially deleted: {msg}")
        } else {
            msg
        }
    })
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

/// PIDs (sorted) of processes that look like `app`: the `exe` link's
/// basename is one of `exec_names`, the `exe` link is the root-joined
/// `location`, or `environ` holds `APPIMAGE=<location>`. Unreadable
/// entries are skipped.
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
        .collect();
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
                target.file_name().is_some_and(|n| exec_names.contains(&n))
                    || location_real.as_deref() == Some(target.as_path())
            });
            let by_env = || {
                appimage_var.as_deref().is_some_and(|var| {
                    fs::read(dir.join("environ"))
                        .is_ok_and(|bytes| bytes.split(|&b| b == 0).any(|kv| kv == var))
                })
            };
            (by_exe || by_env()).then_some(pid)
        })
        .collect();
    pids.sort_unstable();
    pids
}
