//! Running confirmed plans, with the running-app and root refusals.

use super::*;

/// The refusal for an app that is running, from its `is running` line.
pub(super) fn running_refusal(msg: &str) -> String {
    format!("{msg}; close it first or re-run with --force")
}

/// What [`execute_confirmed`] did.
pub struct ExecOutcome {
    /// One per target, in order: `None` for a plan with nothing to do.
    /// Empty when nothing ran.
    pub reports: Vec<Option<ExecReport>>,
    /// 0 when every action succeeded; 1 when an app is running (nothing
    /// ran); 3 when some action failed or the privileged run stopped part
    /// way; 5 when administrator rights were missing or not granted.
    pub exit_code: u8,
    /// With exit 5: the commands to run by hand for what needed the rights.
    pub manual_commands: Vec<String>,
    /// `<name> is running (pid …)`: with exit 1 for each running app that
    /// was not allowed, otherwise for each target app found running.
    pub running: Vec<String>,
    /// With exit 2: why nothing ran (root acting on another user's home).
    pub refusal: Option<String>,
}

/// Why crumbs refuses to act as root here, if it does.
pub fn root_refusal(env: &Env) -> Option<String> {
    env.foreign_home_as_root().map(|home| {
        format!(
            "refusing to act as root on another user's home ({}); run crumbs as that user, or as root with root's own HOME",
            home.display()
        )
    })
}

/// Which target apps may be running while [`execute_confirmed`] runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AllowedRunning {
    /// None: any running target app refuses the run.
    None,
    /// The apps with these ids (the ones the user confirmed).
    Only(Vec<String>),
    /// Any (`--force`).
    Any,
}

impl AllowedRunning {
    pub(super) fn allows(&self, app: &App) -> bool {
        match self {
            AllowedRunning::None => false,
            AllowedRunning::Only(ids) => ids.contains(&app.id),
            AllowedRunning::Any => true,
        }
    }
}

/// Runs `targets` once the user has confirmed them; never prompts. Nothing
/// runs when a privileged action has no way to get the rights, or when a
/// target app is running that `allowed` does not allow. Otherwise the debug
/// before-execute hook, then every plan with [`execute_many`] (Ctrl-C held
/// while a privileged batch runs), which re-checks every action and
/// writes the history.
pub fn execute_confirmed(
    env: &Env,
    cfg: &Config,
    targets: &[Target],
    privilege: Privilege,
    allowed: AllowedRunning,
) -> ExecOutcome {
    let plans: Vec<&Plan> = targets.iter().map(|t| &t.plan).collect();
    let refused = |exit_code: u8, manual_commands: Vec<String>, running: Vec<String>| ExecOutcome {
        reports: Vec::new(),
        exit_code,
        manual_commands,
        running,
        refusal: None,
    };
    if let Some(msg) = root_refusal(env) {
        return ExecOutcome {
            refusal: Some(msg),
            ..refused(2, Vec::new(), Vec::new())
        };
    }
    let needs_rights = plans.iter().flat_map(|p| &p.actions).any(is_privileged);
    if needs_rights && privilege.esc.is_none() {
        let manual = manual_commands(env, plans.iter().flat_map(|p| &p.actions));
        return refused(EXIT_NO_RIGHTS, manual, Vec::new());
    }
    let found = running_apps(env, targets);
    let disallowed: Vec<String> = found
        .iter()
        .filter(|(app, _)| !allowed.allows(app))
        .map(|(_, msg)| msg.clone())
        .collect();
    if !disallowed.is_empty() {
        return refused(1, Vec::new(), disallowed);
    }
    let running: Vec<String> = found.into_iter().map(|(_, msg)| msg).collect();

    before_execute_hook(env);
    let reports = run_plans(env, cfg, &plans, &privilege);
    let all_ok = reports
        .iter()
        .flatten()
        .all(|r| r.results.iter().all(|x| x.ok));
    let not_granted = reports.iter().flatten().any(|r| r.escalation_failed);
    let stopped = reports.iter().flatten().any(|r| r.batch_stopped);
    let (exit_code, manual_commands) = if not_granted {
        (EXIT_NO_RIGHTS, failed_privileged(env, &plans, &reports))
    } else if stopped || !all_ok {
        (3, Vec::new())
    } else {
        (0, Vec::new())
    };
    ExecOutcome {
        reports,
        exit_code,
        manual_commands,
        running,
        refusal: None,
    }
}

/// Runs every plan with [`execute_many`], so all privileged actions of
/// the run share one batch and one prompt; `None` for an empty plan.
pub(super) fn run_plans(
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
/// executes, so a test can change the files between scan and execute. Its
/// stdin is `/dev/null` and its stdout goes to stderr: under `crumbs mcp`
/// stdin and stdout carry the protocol.
pub(super) fn before_execute_hook(env: &Env) {
    #[cfg(debug_assertions)]
    if env.root != Path::new("/")
        && let Some(script) = std::env::var_os("CRUMBS_TEST_BEFORE_EXECUTE")
    {
        let _ = std::process::Command::new("/bin/sh")
            .arg("-c")
            .arg(script)
            .stdin(std::process::Stdio::null())
            .stdout(std::io::stderr())
            .status();
    }
    #[cfg(not(debug_assertions))]
    let _ = env;
}

/// The manual commands for the privileged actions that did not run because
/// administrator rights were missing. An action refused for another reason
/// (a file that changed since the scan) is not among them.
pub(super) fn failed_privileged(
    env: &Env,
    plans: &[&Plan],
    reports: &[Option<ExecReport>],
) -> Vec<String> {
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
