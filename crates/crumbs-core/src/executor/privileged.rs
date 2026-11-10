//! The privileged batch: one escalation for every root step, then its results.

use super::*;

/// Per plan, its privileged attempts with their action indices.
pub(super) type PlanAttempts = Vec<Vec<(usize, Attempt)>>;

/// What [`run_privileged`] did, per plan.
pub(super) struct Privileged {
    pub(super) attempts: PlanAttempts,
    /// Its escalation failed.
    pub(super) failed: Vec<bool>,
    /// The batch stopped part-way through its steps.
    pub(super) stopped: Vec<bool>,
    /// The escalation's captured stderr when the batch gave back no
    /// marker at all; empty otherwise.
    pub(super) unmarked_stderr: Vec<u8>,
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
pub(super) fn run_privileged(
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
pub(super) enum StepEnd {
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
pub(super) fn check_packages(
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
pub(super) fn privileged_step(
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
pub(super) fn exit_message(program: &str, status: i32, line: Option<String>) -> String {
    match line {
        Some(line) => format!("{program} exited with status {status}: {line}"),
        None => format!("{program} exited with status {status}"),
    }
}

/// The last non-empty line of a command's output, trimmed.
pub(super) fn last_line(output: &str) -> Option<String> {
    output
        .lines()
        .rev()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .map(str::to_string)
}
