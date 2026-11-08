//! The flow shared by the commands that change files (`uninstall`,
//! `orphans --clean`, `clean`): usage checks, choosing findings, then
//! preview, running-app check, confirmation, execution and report.

use std::io::{IsTerminal, Write};
use std::path::Path;
use std::process::ExitCode;

use clap::Args;
use crumbs_core::command::{SystemRunner, USER_COMMAND_TIMEOUT};
use crumbs_core::config::Config;
use crumbs_core::executor::{ExecReport, execute_many, running_processes};
use crumbs_core::inventory::Inventory;
use crumbs_core::leftovers::{Finding, FindingKind};
use crumbs_core::plan::{Action, Plan, PlannedAction};
use crumbs_core::privilege::batch::{PrivilegedRunner, SystemPrivilegedRunner};
use crumbs_core::privilege::{Escalation, detect_escalation};
use crumbs_core::removal::still_installed;
use crumbs_core::{App, Env, SourceKind};
use serde_json::json;

use super::context::print_notes;
use super::manual::{is_privileged, manual_commands};
use super::render;
use crate::output::{ErrorEntry, finding_kind, print_json_entries};
use crate::prompt;

/// Flags shared by the commands that change files.
#[derive(Args, Debug, Clone, Default)]
pub struct ActFlags {
    /// Choose the items in a checklist
    #[arg(short = 'i', long)]
    pub interactive: bool,
    /// Show the plan and stop
    #[arg(long)]
    pub dry_run: bool,
    /// Do not ask for confirmation
    #[arg(long)]
    pub yes: bool,
    /// Delete instead of moving to the trash (cannot be undone)
    #[arg(long)]
    pub permanent: bool,
    /// Continue despite failed sources or a running app
    #[arg(long)]
    pub force: bool,
    /// Machine-readable output (needs --yes or --dry-run)
    #[arg(long)]
    pub json: bool,
}

pub const NON_INTERACTIVE: &str = "refusing to act without --yes in a non-interactive session";
pub const NO_RIGHTS: &str = "no way to get administrator rights";
pub const NOT_GRANTED: &str =
    "administrator rights were not granted; nothing that needs them was changed";
pub const BATCH_STOPPED: &str = "the privileged run stopped before it finished; \
     the step it was in may have partly run, and the steps after it did not run";
/// Exit code when administrator rights were needed but not available.
const EXIT_NO_RIGHTS: u8 = 5;

/// Where a command's refusals go: always stderr, and under `--json` also
/// an envelope on stdout with a `cli` error entry.
#[derive(Clone, Copy)]
pub struct Out<'a> {
    pub command: &'a str,
    pub json: bool,
}

impl Out<'_> {
    /// Prints `error: <msg>` and the indented `details`, and returns `code`.
    pub fn fail_with(&self, code: u8, msg: &str, details: &[String]) -> anyhow::Result<ExitCode> {
        self.fail_after(code, msg, details, &[])
    }

    /// [`Self::fail_with`] after the entries that caused it: they are
    /// printed as warnings first and lead the envelope's `errors`.
    fn fail_after(
        &self,
        code: u8,
        msg: &str,
        details: &[String],
        causes: &[ErrorEntry],
    ) -> anyhow::Result<ExitCode> {
        print_notes(causes)?;
        let mut err = anstream::stderr().lock();
        writeln!(err, "error: {}", render::sanitise(msg))?;
        for d in details {
            writeln!(err, "  {}", render::sanitise(d))?;
        }
        drop(err);
        self.envelope(msg, details, causes)?;
        Ok(ExitCode::from(code))
    }

    /// Prints `error: <msg>` and the commands to run by hand, one per
    /// line; under `--json` the envelope's `data` also holds them as
    /// `manual_commands`. Returns exit 5.
    fn no_rights(
        &self,
        msg: &str,
        manual: &[String],
        mut data: serde_json::Value,
        warnings: &[ErrorEntry],
    ) -> anyhow::Result<ExitCode> {
        let mut err = anstream::stderr().lock();
        writeln!(err, "error: {msg}")?;
        if !manual.is_empty() {
            writeln!(err, "to do it yourself, run:")?;
        }
        for m in manual {
            writeln!(err, "  {m}")?;
        }
        drop(err);
        if self.json {
            data["manual_commands"] = json!(manual);
            let mut errors = warnings.to_vec();
            errors.push(ErrorEntry::new("cli", msg.to_string()));
            print_json_entries(self.command, &data, &errors)?;
        }
        Ok(ExitCode::from(EXIT_NO_RIGHTS))
    }

    pub fn fail(&self, code: u8, msg: &str) -> anyhow::Result<ExitCode> {
        self.fail_with(code, msg, &[])
    }

    /// The user said no (exit 4).
    pub fn aborted(&self) -> anyhow::Result<ExitCode> {
        writeln!(anstream::stderr().lock(), "aborted")?;
        self.envelope("aborted", &[], &[])?;
        Ok(ExitCode::from(4))
    }

    fn envelope(&self, msg: &str, details: &[String], causes: &[ErrorEntry]) -> anyhow::Result<()> {
        if !self.json {
            return Ok(());
        }
        let message = if details.is_empty() {
            msg.to_string()
        } else {
            format!("{msg} {}", details.join("; "))
        };
        let mut errors = causes.to_vec();
        errors.push(ErrorEntry::new("cli", message));
        print_json_entries(
            self.command,
            &json!({ "plans": [], "report": null }),
            &errors,
        )
    }
}

/// Usage checks that need no scan. `Some(code)` means stop.
pub fn precheck(out: Out, act: &ActFlags) -> anyhow::Result<Option<ExitCode>> {
    if act.json && !act.yes && !act.dry_run {
        return out.fail(2, "--json needs --yes or --dry-run").map(Some);
    }
    let interactive = prompt::is_interactive();
    if act.interactive && !interactive {
        return out.fail(2, "-i needs an interactive terminal").map(Some);
    }
    if !act.dry_run && !act.yes && !interactive {
        return out.fail(2, NON_INTERACTIVE).map(Some);
    }
    Ok(None)
}

/// Exit 1 when some source failed, unless `--force` or `--dry-run` (a
/// preview only warns).
pub fn refuse_source_errors(
    out: Out,
    inv: &Inventory,
    act: &ActFlags,
) -> anyhow::Result<Option<ExitCode>> {
    let failures: Vec<ErrorEntry> = inv.errors.iter().map(ErrorEntry::from).collect();
    refuse_failed(out, &failures, act)
}

/// [`refuse_source_errors`] for any failures, one entry per failed source:
/// their messages are shown (and under `--json` included) before the
/// refusal.
pub fn refuse_failed(
    out: Out,
    failures: &[ErrorEntry],
    act: &ActFlags,
) -> anyhow::Result<Option<ExitCode>> {
    if failures.is_empty() || act.force || act.dry_run {
        return Ok(None);
    }
    let mut failed: Vec<&str> = failures.iter().map(|e| e.source.as_str()).collect();
    failed.sort_unstable();
    failed.dedup();
    let msg = format!(
        "refusing: some sources failed ({}); re-run with --force to continue",
        failed.join(", ")
    );
    out.fail_after(1, &msg, &[], failures).map(Some)
}

/// Findings a plan may ever contain: a shared `mimeapps.list` edit is
/// never offered.
pub fn plannable(findings: Vec<Finding>) -> Vec<Finding> {
    findings
        .into_iter()
        .filter(|f| !(f.shared && f.kind == FindingKind::MimeAssoc))
        .collect()
}

/// Returns the findings to plan: the default ones, or with `-i` the ones
/// the user ticks. `None` when the checklist was cancelled.
pub fn choose(
    env: &Env,
    prompt_text: &str,
    findings: Vec<Finding>,
    interactive: bool,
    defaults: impl Fn(&Finding) -> bool,
) -> anyhow::Result<Option<Vec<Finding>>> {
    if !interactive {
        return Ok(Some(findings.into_iter().filter(|f| defaults(f)).collect()));
    }
    if findings.is_empty() {
        return Ok(Some(findings));
    }
    let items: Vec<String> = findings
        .iter()
        .map(|f| {
            let shared = if f.shared { "  (shared)" } else { "" };
            format!(
                "{:<12} {:>10}  {}{shared}",
                finding_kind(f.kind),
                render::finding_size(&f.size),
                render::shown_path(env, &f.path)
            )
        })
        .collect();
    let ticks: Vec<bool> = findings.iter().map(&defaults).collect();
    let Some(picked) = prompt::multi_select(prompt_text, &items, &ticks)? else {
        return Ok(None);
    };
    Ok(Some(
        findings
            .into_iter()
            .enumerate()
            .filter(|(i, _)| picked.contains(i))
            .map(|(_, f)| f)
            .collect(),
    ))
}

pub struct Target<'a> {
    pub heading: String,
    /// `None` for orphans.
    pub app: Option<&'a App>,
    pub plan: Plan,
}

/// Makes every path planned once: an action whose path equals or lies
/// under a path an earlier target already plans is dropped. A second edit
/// of the same file is merged into the earlier edit instead, since the
/// first edit replaces the file and the second could never verify.
pub fn drop_overlaps(targets: &mut [Target]) {
    for i in 1..targets.len() {
        let (earlier, rest) = targets.split_at_mut(i);
        let Some(t) = rest.first_mut() else { continue };
        let mut kept: Vec<PlannedAction> = Vec::with_capacity(t.plan.actions.len());
        // Old index -> new index of the actions that stay.
        let mut new_index: Vec<Option<usize>> = Vec::with_capacity(t.plan.actions.len());
        // Old index -> (plan, action) of the earlier action that covers a
        // dropped one.
        let mut cover: Vec<Option<(usize, usize)>> = Vec::with_capacity(t.plan.actions.len());
        for mut pa in t.plan.actions.drain(..) {
            // A command has no filesystem path to overlap with.
            let path = pa.action.path().map(Path::to_path_buf);
            let covered_by = path.as_ref().and_then(|path| {
                earlier.iter().enumerate().find_map(|(ti, e)| {
                    e.plan
                        .actions
                        .iter()
                        .position(|a| a.action.path().is_some_and(|ep| path.starts_with(ep)))
                        .map(|ai| (ti, ai))
                })
            });
            cover.push(covered_by);
            if covered_by.is_none() {
                // A dependency dropped because an earlier plan covers it is
                // waited for there: the executor skips this action if that
                // cover failed.
                if let Some(d) = pa.depends_on {
                    match new_index.get(d).copied().flatten() {
                        Some(n) => pa.depends_on = Some(n),
                        None => {
                            pa.depends_on = None;
                            pa.depends_on_plan = cover.get(d).copied().flatten();
                        }
                    }
                }
                new_index.push(Some(kept.len()));
                kept.push(pa);
                continue;
            }
            new_index.push(None);
            if let Action::EditFile { remove_lines, .. } = &pa.action {
                let first = earlier
                    .iter_mut()
                    .flat_map(|e| e.plan.actions.iter_mut())
                    .map(|e| &mut e.action)
                    .find(|a| a.path() == path.as_deref());
                if let Some(Action::EditFile {
                    remove_lines: lines,
                    ..
                }) = first
                {
                    for l in remove_lines {
                        if !lines.contains(l) {
                            lines.push(l.clone());
                        }
                    }
                }
            }
        }
        t.plan.actions = kept;
        t.plan.total_size = t.plan.actions.iter().map(|a| a.size).sum();
    }
}

/// Steps 4 to 7: preview, running-app check, confirmation, execution and
/// report. `errors` (failed sources, then notes) are warnings in the
/// preview and the envelope's `errors` under `--json`. JSON `report` has
/// one entry per plan, in order: `null` for a plan with nothing to do; the
/// whole field is `null` on a dry run.
pub fn apply(
    env: &Env,
    cfg: &Config,
    errors: &[ErrorEntry],
    out: Out,
    targets: &[Target],
    act: &ActFlags,
) -> anyhow::Result<ExitCode> {
    let plans: Vec<&Plan> = targets.iter().map(|t| &t.plan).collect();
    let empty = plans.iter().all(|p| p.actions.is_empty());

    if !act.json {
        let headed: Vec<(String, &Plan)> = targets
            .iter()
            .map(|t| (t.heading.clone(), &t.plan))
            .collect();
        render::preview(env, &headed)?;
        print_notes(errors)?;
    }
    if act.dry_run {
        if act.json {
            let data = json!({ "plans": plans, "report": null });
            print_json_entries(out.command, &data, errors)?;
        }
        return Ok(ExitCode::SUCCESS);
    }
    if empty {
        if act.json {
            let nulls = vec![serde_json::Value::Null; plans.len()];
            let data = json!({ "plans": plans, "report": nulls });
            print_json_entries(out.command, &data, errors)?;
        } else {
            writeln!(anstream::stdout().lock(), "nothing to do")?;
        }
        return Ok(ExitCode::SUCCESS);
    }

    let needs_rights = plans.iter().flat_map(|p| &p.actions).any(is_privileged);
    let privilege = Privilege::for_run(env, needs_rights);
    if needs_rights && privilege.esc.is_none() {
        let manual = manual_commands(env, plans.iter().flat_map(|p| &p.actions));
        let data = json!({ "plans": plans, "report": null });
        return out.no_rights(NO_RIGHTS, &manual, data, errors);
    }

    let interactive = prompt::is_interactive();
    for app in targets.iter().filter_map(|t| t.app) {
        let pids = running_processes(env, app);
        if pids.is_empty() {
            continue;
        }
        let list: Vec<String> = pids.iter().map(u32::to_string).collect();
        let msg = format!(
            "{} is running (pid {})",
            render::sanitise(&app.display_name),
            list.join(", ")
        );
        writeln!(anstream::stderr().lock(), "{msg}")?;
        if act.force {
            continue;
        }
        if !interactive {
            return out.fail(1, &format!("{msg}; close it first or re-run with --force"));
        }
        if !prompt::confirm(&format!(
            "{} is running; continue anyway?",
            render::sanitise(&app.display_name)
        ))? {
            return out.aborted();
        }
    }
    if !act.yes {
        if !interactive {
            return out.fail(2, NON_INTERACTIVE);
        }
        if !prompt::confirm("Proceed?")? {
            return out.aborted();
        }
    }

    before_execute_hook(env);
    let reports = run_plans(env, cfg, &plans, &privilege);
    let all_ok = reports
        .iter()
        .flatten()
        .all(|r| r.results.iter().all(|x| x.ok));
    let not_granted = reports.iter().flatten().any(|r| r.escalation_failed);
    // The root shell ran and was stopped part-way: some privileged steps
    // ran, so this is not "nothing changed"; the results say which.
    let stopped = reports.iter().flatten().any(|r| r.batch_stopped);
    if act.json {
        let data = json!({ "plans": plans, "report": reports });
        if not_granted {
            let manual = failed_privileged(env, &plans, &reports);
            return out.no_rights(NOT_GRANTED, &manual, data, errors);
        }
        if stopped {
            let mut all = errors.to_vec();
            all.push(ErrorEntry::new("cli", BATCH_STOPPED.to_string()));
            writeln!(anstream::stderr().lock(), "error: {BATCH_STOPPED}")?;
            print_json_entries(out.command, &data, &all)?;
            return Ok(ExitCode::from(3));
        }
        print_json_entries(out.command, &data, errors)?;
    } else {
        writeln!(anstream::stdout().lock())?;
        let runs: Vec<(&Plan, &ExecReport)> = plans
            .iter()
            .zip(&reports)
            .filter_map(|(p, r)| r.as_ref().map(|r| (*p, r)))
            .collect();
        render::report(env, &runs)?;
        if not_granted {
            let manual = failed_privileged(env, &plans, &reports);
            return out.no_rights(NOT_GRANTED, &manual, serde_json::Value::Null, errors);
        }
        if stopped {
            writeln!(anstream::stderr().lock(), "error: {BATCH_STOPPED}")?;
            return Ok(ExitCode::from(3));
        }
    }
    Ok(ExitCode::from(if all_ok { 0 } else { 3 }))
}

/// Runs every plan with [`execute_many`], so all privileged actions of
/// the run share one batch and one prompt; `None` for an empty plan.
fn run_plans(
    env: &Env,
    cfg: &Config,
    plans: &[&Plan],
    privilege: &Privilege,
) -> Vec<Option<ExecReport>> {
    let user_runner = SystemRunner::with_timeout(USER_COMMAND_TIMEOUT);
    // Re-reads what is still installed after a package removal claimed
    // success, with the query runner.
    let post_check = |env: &Env, source: SourceKind, names: &[String]| {
        still_installed(env, &SystemRunner::new(), source, names)
    };
    let owned: Vec<Plan> = plans.iter().map(|p| (*p).clone()).collect();
    // The escalation program shares the terminal: Ctrl-C must not end
    // crumbs before it has read the markers and written the history.
    let _held = owned
        .iter()
        .flat_map(|p| &p.actions)
        .any(is_privileged)
        .then(signals::hold);
    let reports = execute_many(
        env,
        cfg,
        &owned,
        &user_runner,
        privilege.runner.as_ref(),
        privilege.esc.as_ref(),
        &post_check,
    );
    owned
        .iter()
        .zip(reports)
        .map(|(p, r)| (!p.actions.is_empty()).then_some(r))
        .collect()
}

/// Debug builds under a fake root only: runs `CRUMBS_TEST_BEFORE_EXECUTE`
/// with `/bin/sh` after the preview and confirmation and before anything
/// executes, so a test can change the files between scan and execute.
fn before_execute_hook(env: &Env) {
    #[cfg(debug_assertions)]
    if env.root != Path::new("/")
        && let Some(script) = std::env::var_os("CRUMBS_TEST_BEFORE_EXECUTE")
    {
        let _ = std::process::Command::new("/bin/sh")
            .arg("-c")
            .arg(script)
            .status();
    }
    #[cfg(not(debug_assertions))]
    let _ = env;
}

/// The manual commands for the privileged actions that did not run because
/// administrator rights were missing. An action refused for another reason
/// (a file that changed since the scan) is not among them.
fn failed_privileged(env: &Env, plans: &[&Plan], reports: &[Option<ExecReport>]) -> Vec<String> {
    let failed = plans
        .iter()
        .zip(reports)
        .filter_map(|(p, r)| r.as_ref().map(|r| (p, r)))
        .flat_map(|(p, r)| {
            p.actions
                .iter()
                .zip(&r.results)
                .filter(|(_, res)| res.rights_missing)
                .map(|(pa, _)| pa)
        });
    manual_commands(env, failed)
}

/// Whether crumbs can use `esc` now: one that needs a terminal only when
/// stdin is one.
pub fn usable(esc: &Escalation) -> bool {
    !esc.requires_tty || std::io::stdin().is_terminal()
}

/// How this run gets administrator rights: the escalation program (if
/// any) and the runner of the privileged batch.
struct Privilege {
    esc: Option<Escalation>,
    runner: Box<dyn PrivilegedRunner>,
}

impl Privilege {
    /// Detects the escalation program when `needed`. One that needs a
    /// terminal counts only when stdin is one.
    ///
    /// Debug builds never escalate for real unless
    /// `CRUMBS_ALLOW_REAL_ESCALATION=1`. Under a fake root (`CRUMBS_ROOT`)
    /// only, `CRUMBS_TEST_PRIV_RUNNER=sh` runs the batch with `/bin/sh` as
    /// the current user (`deny`: nothing runs, as when the prompt is
    /// dismissed; `partial:<n>`: like `sh`, but the shell stops before step
    /// `n`, as if it were killed); on the real root it is ignored. Release
    /// builds refuse to start when `CRUMBS_TEST_*` is set.
    fn for_run(env: &Env, needed: bool) -> Privilege {
        let system =
            || -> Box<dyn PrivilegedRunner> { Box::new(SystemPrivilegedRunner::new(&env.root)) };
        if !needed {
            return Privilege {
                esc: None,
                runner: system(),
            };
        }
        #[cfg(debug_assertions)]
        {
            let value = std::env::var("CRUMBS_TEST_PRIV_RUNNER").ok();
            if let Some(runner) = test_runner::select(&env.root, value.as_deref()) {
                return Privilege {
                    esc: Some(test_runner::escalation()),
                    runner,
                };
            }
            if std::env::var_os("CRUMBS_ALLOW_REAL_ESCALATION").is_none_or(|v| v != "1") {
                return Privilege {
                    esc: None,
                    runner: system(),
                };
            }
        }
        let esc = detect_escalation(env, &SystemRunner::new()).filter(usable);
        Privilege {
            esc,
            runner: system(),
        }
    }
}

/// Debug-only stand-ins for the escalation, selected with
/// `CRUMBS_TEST_PRIV_RUNNER`.
#[cfg(debug_assertions)]
mod test_runner {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;
    use std::path::{Path, PathBuf};
    use std::process::{Command, Stdio};

    use crumbs_core::CoreError;
    use crumbs_core::privilege::batch::PrivilegedRunner;
    use crumbs_core::privilege::{Escalation, EscalationKind};

    /// Runs the script with `/bin/sh` as the current user.
    struct Sh;

    impl PrivilegedRunner for Sh {
        fn run(&self, _esc: &Escalation, script: &[u8]) -> Result<Vec<u8>, CoreError> {
            // Lets tests count the batches of a run.
            eprintln!("crumbs-test: privileged batch");
            let out = Command::new("/bin/sh")
                .arg("-c")
                .arg(OsStr::from_bytes(script))
                .stdin(Stdio::null())
                .stderr(Stdio::null())
                .output()
                .map_err(|e| CoreError::Command {
                    program: "/bin/sh".into(),
                    message: e.to_string(),
                })?;
            Ok(out.stdout)
        }
    }

    /// [`Sh`] on the script cut just before step `n`'s line: the shell
    /// ends after `n` steps, as if it had been killed during step `n`.
    struct Partial(usize);

    impl PrivilegedRunner for Partial {
        fn run(&self, esc: &Escalation, script: &[u8]) -> Result<Vec<u8>, CoreError> {
            let mut cut = Vec::new();
            for line in script.split_inclusive(|&b| b == b'\n') {
                if step_index(line) == Some(self.0) {
                    break;
                }
                cut.extend_from_slice(line);
            }
            Sh.run(esc, &cut)
        }
    }

    /// The index of a step line (`__c <i> …` or `__t<n> <i> …`).
    fn step_index(line: &[u8]) -> Option<usize> {
        let mut words = line.split(|&b| b == b' ');
        let first = words.next()?;
        let is_step = first == b"__c"
            || first
                .strip_prefix(b"__t")
                .is_some_and(|n| !n.is_empty() && n.iter().all(u8::is_ascii_digit));
        if !is_step {
            return None;
        }
        std::str::from_utf8(words.next()?).ok()?.parse().ok()
    }

    /// Runs nothing and prints nothing, like a dismissed prompt.
    struct Deny;

    impl PrivilegedRunner for Deny {
        fn run(&self, _esc: &Escalation, _script: &[u8]) -> Result<Vec<u8>, CoreError> {
            Ok(Vec::new())
        }
    }

    /// The stand-in that `value` (of `CRUMBS_TEST_PRIV_RUNNER`) names,
    /// only when `root` is a fake root: on the real system the fixture
    /// stand-ins would run real programs as the user.
    pub(super) fn select(root: &Path, value: Option<&str>) -> Option<Box<dyn PrivilegedRunner>> {
        if root == Path::new("/") {
            return None;
        }
        match value? {
            "sh" => Some(Box::new(Sh)),
            "deny" => Some(Box::new(Deny)),
            v => {
                let n = v.strip_prefix("partial:")?.parse().ok()?;
                Some(Box::new(Partial(n)))
            }
        }
    }

    pub(super) fn escalation() -> Escalation {
        Escalation {
            kind: EscalationKind::Run0,
            program: PathBuf::from("/usr/bin/run0"),
            requires_tty: false,
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn test_runner_only_under_a_fake_root() {
            let fake = std::path::Path::new("/tmp/fixture");
            assert!(select(std::path::Path::new("/"), Some("sh")).is_none());
            assert!(select(std::path::Path::new("/"), Some("deny")).is_none());
            assert!(select(fake, Some("sh")).is_some());
            assert!(select(fake, Some("deny")).is_some());
            assert!(select(fake, Some("other")).is_none());
            assert!(select(fake, Some("partial:2")).is_some());
            assert!(select(fake, Some("partial:x")).is_none());
            assert!(select(std::path::Path::new("/"), Some("partial:1")).is_none());
            assert!(select(fake, None).is_none());
        }

        #[test]
        fn step_index_reads_step_lines_only() {
            assert_eq!(step_index(b"__c 3 '/usr/bin/true'\n"), Some(3));
            assert_eq!(step_index(b"__t12 40 'x'\n"), Some(40));
            assert_eq!(step_index(b"__t0() { __c \"$1\" 'x' \"$2\"; }\n"), None);
            assert_eq!(step_index(b"__c() { __i=$1; }\n"), None);
            assert_eq!(step_index(b"export PATH=/usr/bin\n"), None);
        }
    }
}

/// Keeps SIGINT and SIGQUIT from ending crumbs while the privileged batch
/// runs. std has no signal API and rustix offers only its experimental
/// `runtime` one, so this declares libc's `signal` (std already links
/// libc). A no-op handler is installed rather than `SIG_IGN`: a caught
/// signal is reset to its default across `exec`, so the escalation program
/// and the steps still see Ctrl-C, while an ignored one would be inherited.
/// glibc's `signal` sets `SA_RESTART`, and std retries on `EINTR` anyway.
/// A signal that is already ignored is left ignored.
mod signals {
    use std::os::raw::c_int;

    const SIGINT: c_int = 2;
    const SIGQUIT: c_int = 3;
    const SIG_IGN: usize = 1;
    const SIG_ERR: usize = usize::MAX;

    unsafe extern "C" {
        fn signal(signum: c_int, handler: usize) -> usize;
    }

    extern "C" fn ignore(_: c_int) {}

    /// Restores the previous handlers when dropped.
    pub(super) struct Held(Vec<(c_int, usize)>);

    pub(super) fn hold() -> Held {
        let handler = ignore as extern "C" fn(c_int) as usize;
        let mut previous = Vec::new();
        for sig in [SIGINT, SIGQUIT] {
            // SAFETY: `ignore` does nothing, so it is async-signal-safe.
            let old = unsafe { signal(sig, handler) };
            if old == SIG_IGN {
                // Already ignored (e.g. started under `nohup`): keep it so,
                // children included.
                // SAFETY: puts back the disposition `signal` returned.
                unsafe { signal(sig, SIG_IGN) };
            } else if old != SIG_ERR {
                previous.push((sig, old));
            }
        }
        Held(previous)
    }

    impl Drop for Held {
        fn drop(&mut self) {
            for &(sig, old) in &self.0 {
                // SAFETY: `old` is the handler `signal` returned for `sig`.
                unsafe { signal(sig, old) };
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        unsafe extern "C" {
            fn raise(sig: c_int) -> c_int;
        }

        /// Signal dispositions are process-wide and tests run in parallel:
        /// every test that changes them holds this.
        static SIGNAL_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

        fn lock() -> std::sync::MutexGuard<'static, ()> {
            SIGNAL_LOCK
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
        }

        /// The disposition of `sig`, briefly set to ignored to read it.
        fn current(sig: c_int) -> usize {
            // SAFETY: puts the old disposition straight back.
            unsafe {
                let old = signal(sig, SIG_IGN);
                signal(sig, old);
                old
            }
        }

        #[test]
        fn held_signals_do_not_end_the_process() {
            let _guard = lock();
            let before = current(SIGINT);
            let held = hold();
            // SAFETY: SIGINT and SIGQUIT have the no-op handler now.
            assert_eq!(unsafe { raise(SIGINT) }, 0);
            assert_eq!(unsafe { raise(SIGQUIT) }, 0);
            drop(held);
            assert_eq!(current(SIGINT), before, "restored");
        }

        #[test]
        fn ignored_signal_stays_ignored() {
            let _guard = lock();
            // SAFETY: sets SIGQUIT to ignored, restored at the end.
            let original = unsafe { signal(SIGQUIT, SIG_IGN) };
            let held = hold();
            assert_eq!(current(SIGQUIT), SIG_IGN, "no handler installed");
            drop(held);
            assert_eq!(current(SIGQUIT), SIG_IGN);
            // SAFETY: restores what the test found.
            unsafe { signal(SIGQUIT, original) };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crumbs_core::safety::{Fingerprint, Scope};
    use std::path::{Path, PathBuf};

    fn action(a: Action, size: u64) -> PlannedAction {
        PlannedAction {
            action: a,
            size,
            fingerprint: Fingerprint {
                dev: 0,
                ino: 0,
                is_symlink: false,
                ctime: 0,
                ctime_nsec: 0,
            },
            scope: Scope::Home,
            depends_on: None,
            depends_on_plan: None,
            reason: String::new(),
        }
    }

    fn trash(p: &str, size: u64) -> PlannedAction {
        action(Action::Trash { path: p.into() }, size)
    }

    fn edit(p: &str, line: &str) -> PlannedAction {
        action(
            Action::EditFile {
                path: p.into(),
                remove_lines: vec![line.into()],
            },
            0,
        )
    }

    fn target(actions: Vec<PlannedAction>) -> Target<'static> {
        let total_size = actions.iter().map(|a| a.size).sum();
        Target {
            heading: String::new(),
            app: None,
            plan: Plan {
                app_id: None,
                kind: None,
                actions,
                total_size,
                notes: Vec::new(),
            },
        }
    }

    fn paths(t: &Target) -> Vec<PathBuf> {
        t.plan
            .actions
            .iter()
            .filter_map(|a| a.action.path().map(Path::to_path_buf))
            .collect()
    }

    #[test]
    fn drop_overlaps_ignores_commands() {
        let cmd = || {
            action(
                Action::UserCommand {
                    program: "flatpak".into(),
                    args: Vec::new(),
                },
                0,
            )
        };
        let mut ts = [
            target(vec![cmd(), trash("/home/user/flatpak", 1)]),
            target(vec![cmd()]),
        ];
        drop_overlaps(&mut ts);
        assert_eq!(ts[0].plan.actions.len(), 2);
        assert_eq!(ts[1].plan.actions.len(), 1, "a command is never an overlap");
    }

    #[test]
    fn drop_overlaps_plans_each_path_once() {
        let mimeapps = "/home/user/.config/mimeapps.list";
        let mut ts = [
            target(vec![
                trash("/home/user/.config/shared", 10),
                edit(mimeapps, "a=a.desktop"),
            ]),
            target(vec![
                trash("/home/user/.config/shared", 10),
                trash("/home/user/.config/shared/sub", 3),
                trash("/home/user/.config/sharedish", 5),
                edit(mimeapps, "b=b.desktop"),
            ]),
        ];
        drop_overlaps(&mut ts);
        assert_eq!(
            paths(&ts[0]),
            [
                PathBuf::from("/home/user/.config/shared"),
                PathBuf::from(mimeapps)
            ]
        );
        assert_eq!(
            paths(&ts[1]),
            [PathBuf::from("/home/user/.config/sharedish")]
        );
        assert_eq!(ts[1].plan.total_size, 5);
        assert_eq!(ts[0].plan.total_size, 10);
        assert_eq!(
            ts[0].plan.actions[1].action,
            Action::EditFile {
                path: mimeapps.into(),
                remove_lines: vec!["a=a.desktop".into(), "b=b.desktop".into()],
            }
        );
    }

    #[test]
    fn drop_overlaps_remaps_depends_on() {
        let mut dependent = trash("/home/user/.config/Bar", 4);
        dependent.depends_on = Some(0);
        let mut kept_dep = trash("/home/user/.cache/Bar", 2);
        kept_dep.depends_on = Some(0);
        let mut ts = [
            target(vec![trash("/home/user/.local/share/foo", 10)]),
            target(vec![
                trash("/home/user/.local/share/foo/Bar.AppImage", 1),
                dependent,
                kept_dep,
            ]),
            // The file stays here; indexes shift when an earlier action goes.
            target(vec![
                trash("/home/user/.local/share/foo/Baz", 1),
                trash("/home/user/Apps/Q.AppImage", 1),
                {
                    let mut a = trash("/home/user/.config/Q", 1);
                    a.depends_on = Some(1);
                    a
                },
            ]),
        ];
        drop_overlaps(&mut ts);
        assert_eq!(ts[1].plan.actions.len(), 2);
        // Their dependency went to the earlier plan's covering action: the
        // executor waits for it there.
        for a in &ts[1].plan.actions {
            assert_eq!(a.depends_on, None);
            assert_eq!(a.depends_on_plan, Some((0, 0)));
        }
        assert_eq!(ts[2].plan.actions.len(), 2);
        assert_eq!(ts[2].plan.actions[1].depends_on, Some(0));
        assert_eq!(ts[2].plan.actions[1].depends_on_plan, None);
    }
}
