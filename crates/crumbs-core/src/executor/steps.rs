//! Doing one action, recording it in the history and explaining skips.

use super::*;

/// Writes the attempt's history entry and returns its result.
pub(super) fn record(
    env: &Env,
    plan: &Plan,
    batch: &str,
    i: usize,
    attempt: Attempt,
) -> ActionResult {
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
pub(super) fn action_target(action: &Action) -> PathBuf {
    match action {
        Action::UserCommand { program, .. }
        | Action::PrivilegedCommand { program, .. }
        | Action::PackageRemove { program, .. } => PathBuf::from(program),
        other => other.path().map(Path::to_path_buf).unwrap_or_default(),
    }
}

pub(super) fn human_duration(d: std::time::Duration) -> String {
    match d.as_secs() {
        s if s >= 60 && s % 60 == 0 => format!("{} min", s / 60),
        s => format!("{s} s"),
    }
}

/// Runs a `UserCommand`. On return `path` is the in-system program path (or
/// the bare name when it is not installed) and `argv` its arguments.
pub(super) fn run_command(
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
pub(super) fn is_build_tree(path: &Path) -> bool {
    path.parent() == Some(Path::new("/var/tmp"))
}

/// Scope rules beyond `verify` that are re-run right before acting: a
/// `UserTemp` entry must still be stale (old, unused, nothing open); a
/// `SystemTree` target must pass [`recheck_system_tree`].
pub(super) fn recheck_scope(
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
pub(super) fn skip_reason(
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
pub(super) fn cross_skip_reason(
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
pub(super) fn shown_action(env: &Env, plan: &Plan, index: usize) -> String {
    plan.actions.get(index).map_or_else(
        || format!("action {index}"),
        |d| display_path(env, &action_target(&d.action)),
    )
}

pub(super) fn io_msg(env: &Env, in_system: &Path, e: &io::Error) -> String {
    format!("{}: {e}", display_path(env, in_system))
}

/// Acts on an already-verified path.
pub(super) fn act(
    env: &Env,
    action: &Action,
    fp: &Fingerprint,
    batch: &str,
) -> Result<Done, String> {
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
