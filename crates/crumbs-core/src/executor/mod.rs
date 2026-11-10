//! Runs a [`Plan`]: every action is re-verified with `safety::verify`
//! immediately before it acts, on exactly the verified path, with no-follow
//! operations. Failures are recorded and the run continues.

mod checks;
mod delete;
pub(crate) mod files;
mod privileged;
mod processes;
mod steps;
#[cfg(test)]
mod tests;
mod tree;
pub use checks::*;
use delete::*;
use privileged::*;
pub use processes::*;
use steps::*;
pub use tree::*;

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
