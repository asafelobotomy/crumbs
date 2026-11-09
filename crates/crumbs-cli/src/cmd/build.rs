//! The commands without their printing: what each would plan or report,
//! or why it refuses. The CLI prints what these return; the `mcp` server
//! returns it to the agent. Nothing here prints or prompts.
//!
//! The public functions are what the `mcp` module calls; the CLI calls the
//! commands' own builders, which can also show a checklist (`-i`).

use std::path::{Path, PathBuf};

use crumbs_core::config::Config;
use crumbs_core::leftovers::Finding;
use crumbs_core::{CoreError, Env, SourceKind};
use serde::Serialize;

use super::act::Target;
use super::apps::{self, Sort};
use super::clean::{self, CleanArgs};
use super::orphans::{self, OrphansArgs};
use super::restore as restore_cmd;
use super::scan::{self, ScanArgs};
use super::uninstall::{self, UninstallArgs};
use super::{doctor, history};
use crate::output::ErrorEntry;

/// What a command that changes files would do.
pub struct Built {
    /// The plans, in order, each under its preview heading.
    pub targets: Vec<Target>,
    /// Failed sources, then notes: warnings in the preview and the JSON
    /// envelope's `errors`.
    pub errors: Vec<ErrorEntry>,
    /// Extra `note:` lines of the human preview.
    pub after_preview: Vec<String>,
    /// Every finding the request could pick from with `select`.
    #[allow(dead_code, reason = "used by the mcp module in a later task")]
    pub selectable: Vec<Finding>,
    /// The argv of the equivalent CLI command.
    #[allow(dead_code, reason = "used by the mcp module in a later task")]
    pub cli_command: Vec<String>,
}

/// Why a command will not go ahead: what the CLI prints as `error:
/// <message>`, with `details` as the indented lines under it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal {
    pub exit_code: u8,
    pub message: String,
    pub details: Vec<String>,
}

impl Refusal {
    pub fn new(exit_code: u8, message: impl Into<String>) -> Refusal {
        Refusal {
            exit_code,
            message: message.into(),
            details: Vec::new(),
        }
    }

    pub fn with_details(mut self, details: Vec<String>) -> Refusal {
        self.details = details;
        self
    }
}

/// Why a command's builder stopped, as the CLI reports it.
#[derive(Debug)]
pub(crate) enum Stop {
    /// `causes` are the failures behind it (failed sources): the CLI shows
    /// them as warnings first.
    Refused {
        refusal: Refusal,
        causes: Vec<ErrorEntry>,
    },
    /// The user cancelled a checklist.
    Aborted,
    /// An error `main` reports (an unreadable config, an I/O error).
    Failed(anyhow::Error),
}

impl Stop {
    /// As a [`Refusal`] for a caller that cannot print the causes: they
    /// become `source: message` details.
    pub(crate) fn into_refusal(self) -> Refusal {
        match self {
            Stop::Refused { refusal, causes } => {
                let mut details = refusal.details;
                details.extend(
                    causes
                        .iter()
                        .map(|c| format!("{}: {}", c.source, c.message)),
                );
                Refusal { details, ..refusal }
            }
            Stop::Aborted => Refusal::new(4, "aborted"),
            Stop::Failed(e) => Refusal::new(1, format!("{e:#}")),
        }
    }

    /// As the error a command without refusals of its own hands to `main`.
    pub(crate) fn into_error(self) -> anyhow::Error {
        match self {
            Stop::Failed(e) => e,
            other => anyhow::anyhow!(other.into_refusal().message),
        }
    }
}

impl From<Refusal> for Stop {
    fn from(refusal: Refusal) -> Stop {
        Stop::Refused {
            refusal,
            causes: Vec::new(),
        }
    }
}

impl From<anyhow::Error> for Stop {
    fn from(e: anyhow::Error) -> Stop {
        Stop::Failed(e)
    }
}

impl From<CoreError> for Stop {
    fn from(e: CoreError) -> Stop {
        Stop::Failed(e.into())
    }
}

/// Which items a builder plans.
pub(crate) enum Pick<'a, T> {
    /// The CLI's defaults.
    Defaults,
    /// Exactly these, each one something the builder found.
    Named(&'a [T]),
    /// Those the user ticks in a checklist (`-i`).
    Checklist,
}

impl<T> Clone for Pick<'_, T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T> Copy for Pick<'_, T> {}

/// A builder's result: what the request would do, plus what only the CLI
/// needs.
pub(crate) struct Draft {
    pub built: Built,
    pub cfg: Config,
    /// `clean` only: the items a scanner held back (their app runs).
    pub held: Vec<String>,
}

/// `p` with a leading `~` as the (in-system) home.
pub(crate) fn expand_home(env: &Env, p: &Path) -> PathBuf {
    match p.strip_prefix("~") {
        Ok(rest) if rest.as_os_str().is_empty() => env.unsys(&env.home),
        Ok(rest) => env.unsys(&env.home).join(rest),
        Err(_) => p.to_path_buf(),
    }
}

/// The refusal for a `select` value crumbs did not find.
pub(crate) fn not_found(value: &str) -> Refusal {
    Refusal::new(2, format!("not found by crumbs: {value}"))
}

/// The in-system paths `select` names, each the path of one of `found`.
pub(crate) fn select_findings(
    env: &Env,
    found: &[Finding],
    select: &[PathBuf],
) -> Result<Vec<PathBuf>, Refusal> {
    let mut wanted = Vec::new();
    for p in select {
        let path = expand_home(env, p);
        if !found.iter().any(|f| f.path == path) {
            return Err(not_found(&p.display().to_string()));
        }
        if !wanted.contains(&path) {
            wanted.push(path);
        }
    }
    Ok(wanted)
}

#[cfg_attr(
    not(feature = "mcp"),
    allow(dead_code, reason = "only the mcp server calls it")
)]
fn value<T: Serialize>(v: &T) -> Result<serde_json::Value, Refusal> {
    serde_json::to_value(v).map_err(|e| Refusal::new(1, e.to_string()))
}

fn failed(e: anyhow::Error) -> Refusal {
    Stop::Failed(e).into_refusal()
}

/// `uninstall`: the apps in `args.apps` and their leftovers. With
/// `select`, exactly the findings at those paths (system leftovers too).
#[allow(dead_code, reason = "used by the mcp module in a later task")]
pub fn uninstall(
    env: &Env,
    args: &UninstallArgs,
    select: Option<&[PathBuf]>,
) -> Result<Built, Refusal> {
    let pick = select.map_or(Pick::Defaults, Pick::Named);
    uninstall::build(env, args, pick)
        .map(|d| d.built)
        .map_err(Stop::into_refusal)
}

/// `clean`: the chosen categories' items. With `select`, exactly the
/// named items: a category id stands for its items that are not held back.
#[allow(dead_code, reason = "used by the mcp module in a later task")]
pub fn clean(env: &Env, args: &CleanArgs, select: Option<&[String]>) -> Result<Built, Refusal> {
    let pick = select.map_or(Pick::Defaults, Pick::Named);
    clean::build(env, args, pick)
        .map(|d| d.built)
        .map_err(Stop::into_refusal)
}

/// `orphans --clean` of the orphans at `select`.
#[allow(dead_code, reason = "used by the mcp module in a later task")]
pub fn orphans(env: &Env, args: &OrphansArgs, select: &[PathBuf]) -> Result<Built, Refusal> {
    orphans::build_clean(env, args, Pick::Named(select))
        .map(|d| d.built)
        .map_err(Stop::into_refusal)
}

/// `restore`: checks the selectors with a dry run. The core restores
/// without a plan, so `targets` is empty; refused when no selector names a
/// history entry.
#[allow(dead_code, reason = "used by the mcp module in a later task")]
pub fn restore(env: &Env, selectors: &[String]) -> Result<Built, Refusal> {
    let (outcomes, notes) = restore_cmd::outcomes(env, selectors, true).map_err(failed)?;
    if restore_cmd::none_selected(&outcomes) {
        let details = outcomes
            .iter()
            .map(|o| {
                format!(
                    "{}: {}",
                    o.path.display(),
                    o.error.as_deref().unwrap_or("failed")
                )
            })
            .collect();
        return Err(Refusal::new(2, "no history entry matches").with_details(details));
    }
    let mut cli_command = vec!["crumbs".to_string(), "restore".to_string()];
    cli_command.extend(selectors.iter().cloned());
    Ok(Built {
        targets: Vec::new(),
        errors: notes,
        after_preview: Vec::new(),
        selectable: Vec::new(),
        cli_command,
    })
}

/// `scan --json` data and errors.
#[cfg_attr(
    not(feature = "mcp"),
    allow(dead_code, reason = "only the mcp server calls it")
)]
pub fn scan_data(
    env: &Env,
    args: &ScanArgs,
) -> Result<(serde_json::Value, Vec<ErrorEntry>), Refusal> {
    let scanned = scan::gather(env, args).map_err(Stop::into_refusal)?;
    Ok((value(&scanned.report)?, scanned.json_errors()))
}

/// `apps list --json` data and errors.
#[cfg_attr(
    not(feature = "mcp"),
    allow(dead_code, reason = "only the mcp server calls it")
)]
pub fn apps_list_data(
    env: &Env,
    sources: &[SourceKind],
    all: bool,
    sort: Sort,
) -> Result<(serde_json::Value, Vec<ErrorEntry>), Refusal> {
    let listed = apps::gather_list(env, sources, all, sort).map_err(failed)?;
    Ok((listed.json_data(), listed.json_errors()))
}

/// `apps show --json` data and errors for a found app; no app or several
/// are refused, as the CLI's exit 2.
#[cfg_attr(
    not(feature = "mcp"),
    allow(dead_code, reason = "only the mcp server calls it")
)]
pub fn app_show_data(
    env: &Env,
    query: &str,
    show_low: bool,
) -> Result<(serde_json::Value, Vec<ErrorEntry>), Refusal> {
    let (cx, shown) = apps::gather_show(env, query, show_low).map_err(failed)?;
    match shown {
        apps::Shown::Found { app, leftovers } => Ok((
            serde_json::json!({ "app": app, "leftovers": leftovers }),
            cx.errors(),
        )),
        apps::Shown::NotFound => Err(Refusal::new(
            2,
            format!("no installed app matches '{query}'"),
        )),
        apps::Shown::Ambiguous(candidates) => {
            Err(
                Refusal::new(2, format!("'{query}' matches several apps:")).with_details(
                    candidates
                        .iter()
                        .map(|a| format!("{}  {}  {}", a.id, a.display_name, a.source.as_str()))
                        .collect(),
                ),
            )
        }
    }
}

/// `orphans --json` data and errors (the listing, not `--clean`).
#[cfg_attr(
    not(feature = "mcp"),
    allow(dead_code, reason = "only the mcp server calls it")
)]
pub fn orphans_data(
    env: &Env,
    args: &OrphansArgs,
) -> Result<(serde_json::Value, Vec<ErrorEntry>), Refusal> {
    if args.user && args.system {
        return Err(Refusal::new(2, orphans::USER_AND_SYSTEM));
    }
    let (cx, found) = orphans::find_listed(env, args).map_err(failed)?;
    Ok((serde_json::json!({ "orphans": found }), cx.errors()))
}

/// `history --json` data and errors: the newest `limit` batches.
#[cfg_attr(
    not(feature = "mcp"),
    allow(dead_code, reason = "only the mcp server calls it")
)]
pub fn history_data(
    env: &Env,
    limit: usize,
) -> Result<(serde_json::Value, Vec<ErrorEntry>), Refusal> {
    history::data(env, limit).map_err(failed)
}

/// `doctor --json` data and errors.
#[cfg_attr(
    not(feature = "mcp"),
    allow(dead_code, reason = "only the mcp server calls it")
)]
pub fn doctor_data(env: &Env) -> Result<(serde_json::Value, Vec<ErrorEntry>), Refusal> {
    let (report, notes) = doctor::gather(env).map_err(failed)?;
    let errors = doctor::json_errors(&report, notes);
    Ok((value(&report)?, errors))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cmd::act::{ActFlags, preview_text};
    use crumbs_core::testing::FixtureTree;

    fn inky() -> FixtureTree {
        let t = FixtureTree::new();
        t.os_release("manjaro", "arch", "Manjaro Linux");
        t.pacman_pkg(
            "inky",
            "1.0-1",
            Some(1000),
            &[
                "usr/bin/inky",
                "usr/share/applications/",
                "usr/share/applications/inky.desktop",
            ],
        );
        t.write_exec("/usr/bin/inky", b"");
        t.desktop(
            "/usr/share/applications/inky.desktop",
            "[Desktop Entry]\nType=Application\nName=Inky\nExec=/usr/bin/inky\n",
        );
        t.write("home/user/.cache/inky/blob", b"cache");
        t.write("home/user/.config/inky/inky.conf", b"x");
        t
    }

    fn leftovers_only(app: &str) -> UninstallArgs {
        UninstallArgs {
            apps: vec![app.into()],
            leftovers_only: true,
            keep_data: false,
            show_low: false,
            act: ActFlags {
                dry_run: true,
                json: true,
                ..ActFlags::default()
            },
        }
    }

    /// What `uninstall inky --leftovers-only` plans in [`inky`]: also
    /// pinned for the CLI in `tests/cli.rs`
    /// (`uninstall_dry_run_json_plan_matches_builder`).
    const INKY_LEFTOVERS: [(&str, &str); 2] = [
        ("trash", "/home/user/.cache/inky"),
        ("trash", "/home/user/.config/inky"),
    ];

    fn actions(plan: &serde_json::Value) -> Vec<(String, String)> {
        let mut v: Vec<(String, String)> = plan["actions"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| {
                let kind = a["action"]["type"].as_str().unwrap().to_string();
                let path = a["action"]["path"].as_str().unwrap().to_string();
                (kind, path)
            })
            .collect();
        v.sort();
        v
    }

    #[test]
    fn build_uninstall_matches_cli_plan() {
        let t = inky();
        let env = t.env();
        let args = leftovers_only("inky");
        let built = uninstall(&env, &args, None).unwrap();
        // The CLI's own builder and the data `--dry-run --json` prints.
        let cli = uninstall::build(&env, &args, Pick::Defaults).unwrap();
        let data = crate::cmd::act::dry_run_data(&cli.built.targets);
        let plan = serde_json::to_value(&built.targets[0].plan).unwrap();
        assert_eq!(plan, data["plans"][0]);
        let want: Vec<(String, String)> = INKY_LEFTOVERS
            .iter()
            .map(|(k, p)| (k.to_string(), p.to_string()))
            .collect();
        assert_eq!(actions(&plan), want, "{plan:#}");
        assert_eq!(
            built.cli_command,
            ["crumbs", "uninstall", "inky", "--leftovers-only"]
        );
    }

    #[test]
    fn build_select_rejects_unknown_path() {
        let t = inky();
        let select = [PathBuf::from("/etc/passwd")];
        let Err(refusal) = uninstall(&t.env(), &leftovers_only("inky"), Some(&select)) else {
            panic!("not refused");
        };
        assert_eq!(refusal.exit_code, 2);
        assert!(
            refusal.message.starts_with("not found by crumbs:"),
            "{}",
            refusal.message
        );
    }

    #[test]
    fn build_select_plans_exactly_the_named_findings() {
        let t = inky();
        let select = [PathBuf::from("~/.cache/inky")];
        let built = uninstall(&t.env(), &leftovers_only("inky"), Some(&select)).unwrap();
        let paths: Vec<_> = built.targets[0]
            .plan
            .actions
            .iter()
            .filter_map(|a| a.action.path())
            .collect();
        assert_eq!(paths, [Path::new("/home/user/.cache/inky")]);
        assert!(built.selectable.len() > 1);
        assert_eq!(built.cli_command.last().map(String::as_str), Some("-i"));
    }

    #[test]
    fn preview_text_has_no_escape_codes() {
        let t = inky();
        let env = t.env();
        let built = uninstall(&env, &leftovers_only("inky"), None).unwrap();
        let errors = [ErrorEntry::new("rules", "a note".into())];
        let text = preview_text(&env, &built.targets, &built.after_preview, &errors);
        assert!(text.contains("ACTION"), "{text}");
        assert!(text.ends_with("warning: rules: a note\n"), "{text}");
        assert!(!text.contains('\x1b'), "{text:?}");
    }

    #[test]
    fn execute_confirmed_refuses_an_app_that_was_not_allowed() {
        use crate::cmd::act::{AllowedRunning, Privilege, execute_confirmed};
        let t = inky();
        t.pacman_pkg(
            "blot",
            "1.0-1",
            Some(1000),
            &[
                "usr/bin/blot",
                "usr/share/applications/",
                "usr/share/applications/blot.desktop",
            ],
        );
        t.write_exec("/usr/bin/blot", b"");
        t.desktop(
            "/usr/share/applications/blot.desktop",
            "[Desktop Entry]\nType=Application\nName=Blot\nExec=/usr/bin/blot\n",
        );
        t.write("home/user/.cache/blot/blob", b"cache");
        // The user confirmed Inky, which was running.
        t.symlink("/proc/79/exe", "/usr/bin/inky");
        let env = t.env();
        let mut args = leftovers_only("inky");
        args.apps.push("blot".into());
        let built = uninstall(&env, &args, None).unwrap();
        let inky_id = built.targets[0].app.as_ref().unwrap().id.clone();
        let cfg = Config::default();
        let caches = [
            env.sys("/home/user/.cache/inky"),
            env.sys("/home/user/.cache/blot"),
        ];
        let run = |allowed| {
            execute_confirmed(
                &env,
                &cfg,
                &built.targets,
                Privilege::for_run(&env, false),
                allowed,
            )
        };
        let only_inky = || AllowedRunning::Only(vec![inky_id.clone()]);

        // Blot starts after the confirmation.
        t.symlink("/proc/80/exe", "/usr/bin/blot");
        let refused = run(only_inky());
        assert_eq!(refused.exit_code, 1);
        assert!(refused.reports.is_empty());
        assert_eq!(refused.running, ["Blot is running (pid 80)"]);
        let none = run(AllowedRunning::None);
        assert_eq!(none.exit_code, 1);
        assert_eq!(
            none.running,
            ["Inky is running (pid 79)", "Blot is running (pid 80)"]
        );
        assert!(caches.iter().all(|c| c.exists()), "nothing ran");

        // Blot is gone again: only the allowed Inky runs.
        std::fs::remove_file(t.dir.path().join("proc/80/exe")).unwrap();
        let ran = run(only_inky());
        assert_eq!(ran.exit_code, 0, "{:?}", ran.reports);
        assert_eq!(ran.running, ["Inky is running (pid 79)"]);
        assert_eq!(ran.reports.len(), 2);
        assert!(caches.iter().all(|c| !c.exists()));
    }

    #[test]
    fn execute_confirmed_runs_with_any_running_app_allowed() {
        use crate::cmd::act::{AllowedRunning, Privilege, execute_confirmed};
        let t = inky();
        t.symlink("/proc/79/exe", "/usr/bin/inky");
        let env = t.env();
        let built = uninstall(&env, &leftovers_only("inky"), None).unwrap();
        let cfg = Config::default();
        let privilege = Privilege::for_run(&env, false);
        let ran = execute_confirmed(&env, &cfg, &built.targets, privilege, AllowedRunning::Any);
        assert_eq!(ran.exit_code, 0, "{:?}", ran.reports);
        assert_eq!(ran.running, ["Inky is running (pid 79)"]);
        assert!(!env.sys("/home/user/.cache/inky").exists());
    }
}
