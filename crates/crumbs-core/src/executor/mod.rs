//! Runs a [`Plan`]: every action is re-verified with `safety::verify`
//! immediately before it acts, on exactly the verified path, with no-follow
//! operations. Failures are recorded and the run continues.

pub(crate) mod files;
#[cfg(test)]
mod tests;

use std::ffi::OsStr;
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
use crate::model::App;
use crate::plan::{Action, Plan};
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
}

/// What a successful action leaves for the history entry.
#[derive(Default)]
struct Done {
    trashed: Option<Trashed>,
    backup: Option<PathBuf>,
}

/// Runs every action in order. An action whose `depends_on` action did not
/// succeed is skipped: `ok: false`, with the reason, and recorded like any
/// other. Each attempt becomes one history entry,
/// appended (and synced) right after the attempt, so an interrupted run
/// still leaves a record of what it did. When an entry can't be written,
/// that result's error gets `"<append error>; history not written"` (its
/// `ok` still describes what happened on disk); other entries are
/// unaffected.
pub fn execute(env: &Env, cfg: &Config, plan: &Plan) -> ExecReport {
    execute_with(
        env,
        cfg,
        plan,
        &SystemRunner::with_timeout(USER_COMMAND_TIMEOUT),
    )
}

/// [`execute`] with the command runner injected; `UserCommand` actions run
/// through it.
pub fn execute_with(
    env: &Env,
    cfg: &Config,
    plan: &Plan,
    runner: &dyn CommandRunner,
) -> ExecReport {
    let batch = history::next_batch_id(env);
    let mut results = Vec::with_capacity(plan.actions.len());
    let mut freed = 0u64;
    for (i, pa) in plan.actions.iter().enumerate() {
        let id = format!("{batch}-{i}");
        let mut path = action_target(&pa.action);
        let mut argv = None;
        let outcome = match skip_reason(env, plan, &results, pa.depends_on) {
            Some(reason) => Err(reason),
            // A command has no path to verify; its program is resolved in
            // the trusted directories right now instead.
            None if matches!(pa.action, Action::UserCommand { .. }) => {
                run_command(env, &pa.action, runner, &mut path, &mut argv)
            }
            None => safety::verify(env, cfg, pa.scope, &path, &pa.fingerprint)
                .map_err(|e| e.to_string())
                .and_then(|()| recheck_scope(env, cfg, pa.scope, &path))
                .and_then(|()| act(env, &pa.action, &pa.fingerprint, &batch)),
        };
        let (ok, error, done) = match outcome {
            Ok(done) => (true, None, done),
            Err(e) => (false, Some(e), Done::default()),
        };
        if ok {
            freed = freed.saturating_add(pa.size);
        }
        let entry = HistoryEntry {
            id: id.clone(),
            batch: batch.clone(),
            time: history::now_rfc3339(),
            action: match pa.action {
                Action::Trash { .. } => HistoryAction::Trash,
                Action::Delete { .. } => HistoryAction::Delete,
                Action::EditFile { .. } => HistoryAction::EditFile,
                Action::UserCommand { .. } => HistoryAction::Command,
            },
            path,
            trashed: done.trashed,
            backup: done.backup,
            size: pa.size,
            ok,
            error: error.clone(),
            app_id: plan.app_id.clone(),
            restores: None,
            argv,
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
        results.push(ActionResult {
            action: pa.action.clone(),
            ok,
            error,
            history_id: id,
        });
    }
    ExecReport {
        batch,
        results,
        freed,
    }
}

/// What stands for the action in results and history: its path, or for a
/// command its program name.
fn action_target(action: &Action) -> PathBuf {
    match action {
        Action::UserCommand { program, .. } => PathBuf::from(program),
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
    Err(out
        .stderr
        .lines()
        .rev()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .map_or_else(
            || format!("exited with status {}", out.status),
            str::to_string,
        ))
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
/// was itself skipped.
fn skip_reason(
    env: &Env,
    plan: &Plan,
    results: &[ActionResult],
    depends_on: Option<usize>,
) -> Option<String> {
    let dep = depends_on?;
    if results.get(dep).is_some_and(|r| r.ok) {
        return None;
    }
    let shown = plan.actions.get(dep).map_or_else(
        || format!("action {dep}"),
        |d| display_path(env, &action_target(&d.action)),
    );
    Some(format!("skipped: {shown} was not removed"))
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
        Action::UserCommand { .. } => Err("a command cannot be run as a path action".into()),
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
