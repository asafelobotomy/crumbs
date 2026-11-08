//! `uninstall`, plus the confirm-and-execute flow `orphans --clean` shares.

use std::io::Write;
use std::process::ExitCode;

use clap::Args;
use crumbs_core::env::display_path;
use crumbs_core::executor::{execute, running_processes};
use crumbs_core::inventory::{Inventory, Resolved, find_app, find_app_exact};
use crumbs_core::leftovers::{Finding, FindingKind, find_leftovers};
use crumbs_core::plan::{NOTE_APP_STAYS, Plan, PlanOptions, build_plan};
use crumbs_core::{App, Env, SourceKind};
use serde_json::json;

use super::context::{Context, print_notes};
use super::render;
use crate::output::{ErrorEntry, finding_kind, fmt_size, print_json_entries};
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

#[derive(Args, Debug)]
pub struct UninstallArgs {
    /// App ids, names, desktop ids or display names (exact matches only)
    #[arg(required = true)]
    pub apps: Vec<String>,
    /// Keep the app itself; remove only its leftovers
    #[arg(long)]
    pub leftovers_only: bool,
    /// Keep data and settings; remove only caches
    #[arg(long)]
    pub keep_data: bool,
    /// Include low-confidence leftovers
    #[arg(long)]
    pub show_low: bool,
    #[command(flatten)]
    pub act: ActFlags,
}

pub const NON_INTERACTIVE: &str = "refusing to act without --yes in a non-interactive session";

fn fail(code: u8, msg: &str) -> anyhow::Result<ExitCode> {
    writeln!(anstream::stderr().lock(), "error: {msg}")?;
    Ok(ExitCode::from(code))
}

/// Usage checks that need no scan. `Some(code)` means stop.
pub fn precheck(act: &ActFlags) -> anyhow::Result<Option<ExitCode>> {
    if act.json && !act.yes && !act.dry_run {
        return fail(2, "--json needs --yes or --dry-run").map(Some);
    }
    let interactive = prompt::is_interactive();
    if act.interactive && !interactive {
        return fail(2, "-i needs an interactive terminal").map(Some);
    }
    if !act.dry_run && !act.yes && !interactive {
        return fail(2, NON_INTERACTIVE).map(Some);
    }
    Ok(None)
}

/// Exit 1 when some source failed and `--force` is not given.
pub fn refuse_source_errors(inv: &Inventory, force: bool) -> anyhow::Result<Option<ExitCode>> {
    if inv.errors.is_empty() || force {
        return Ok(None);
    }
    let mut failed: Vec<&str> = inv.errors.iter().map(|e| e.source.as_str()).collect();
    failed.sort_unstable();
    failed.dedup();
    let msg = format!(
        "refusing: some sources failed ({}); re-run with --force to continue",
        failed.join(", ")
    );
    fail(1, &msg).map(Some)
}

/// Findings a plan may ever contain: a shared `mimeapps.list` edit is
/// never offered.
pub fn plannable(findings: Vec<Finding>) -> Vec<Finding> {
    findings
        .into_iter()
        .filter(|f| !(f.shared && f.kind == FindingKind::MimeAssoc))
        .collect()
}

/// Returns the findings to plan: the preselected ones, or with `-i` the
/// ones the user ticks. `None` when the checklist was cancelled.
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
                fmt_size(f.size.bytes),
                display_path(env, &f.path)
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

pub fn run(env: &Env, args: &UninstallArgs) -> anyhow::Result<ExitCode> {
    if let Some(code) = precheck(&args.act)? {
        return Ok(code);
    }
    let cx = Context::load(env)?;
    let mut apps: Vec<&App> = Vec::new();
    for query in &args.apps {
        match resolve(&cx.inv, query)? {
            Ok(app) => {
                if !apps.iter().any(|a| a.id == app.id) {
                    apps.push(app);
                }
            }
            Err(code) => return Ok(code),
        }
    }
    if !args.leftovers_only
        && let Some(app) = apps.iter().find(|a| a.source != SourceKind::AppImage)
    {
        let msg = format!(
            "removing {} apps needs privileged support (coming in a later version); use --leftovers-only",
            app.source.as_str()
        );
        return fail(2, &msg);
    }
    if let Some(code) = refuse_source_errors(&cx.inv, args.act.force)? {
        return Ok(code);
    }
    let opts = PlanOptions {
        permanent: args.act.permanent,
        keep_data: args.keep_data,
    };
    let mut targets = Vec::new();
    for app in apps {
        let found = plannable(find_leftovers(&cx.idx, app, args.show_low));
        let prompt_text = format!("Leftovers of {} to remove", app.display_name);
        let Some(chosen) = choose(env, &prompt_text, found, args.act.interactive, |f| {
            f.preselected
        })?
        else {
            return aborted();
        };
        let mut plan = build_plan(env, Some(app), &chosen, &opts)?;
        if args.leftovers_only && app.source == SourceKind::AppImage {
            keep_appimage(&mut plan, app);
        }
        targets.push(Target {
            heading: format!("{}  {}", app.display_name, app.id),
            app: Some(app),
            plan,
        });
    }
    apply(env, &cx, "uninstall", &targets, &args.act)
}

/// Drops the AppImage file itself from the plan.
fn keep_appimage(plan: &mut Plan, app: &App) {
    plan.actions
        .retain(|a| Some(a.action.path()) != app.location.as_deref());
    plan.total_size = plan.actions.iter().map(|a| a.size).sum();
    plan.notes.push(NOTE_APP_STAYS.into());
}

/// The app for one query (exact matches only), or the exit code after
/// explaining why not.
fn resolve<'a>(inv: &'a Inventory, query: &str) -> anyhow::Result<Result<&'a App, ExitCode>> {
    let mut err = anstream::stderr().lock();
    let candidates = match find_app_exact(inv, query) {
        Resolved::Found(app) => return Ok(Ok(app)),
        Resolved::Ambiguous(c) => {
            writeln!(err, "error: '{query}' matches several apps:")?;
            c
        }
        Resolved::NotFound => match find_app(inv, query) {
            Resolved::Found(app) => {
                writeln!(
                    err,
                    "error: no app is exactly '{query}'; did you mean '{}'?",
                    app.id
                )?;
                return Ok(Err(ExitCode::from(2)));
            }
            Resolved::Ambiguous(c) => {
                writeln!(
                    err,
                    "error: no app is exactly '{query}'; did you mean one of these?"
                )?;
                c
            }
            Resolved::NotFound => {
                writeln!(err, "error: no installed app matches '{query}'")?;
                return Ok(Err(ExitCode::from(2)));
            }
        },
    };
    for a in candidates {
        writeln!(err, "  {}  {}  {}", a.id, a.display_name, a.source.as_str())?;
    }
    Ok(Err(ExitCode::from(2)))
}

pub struct Target<'a> {
    pub heading: String,
    /// `None` for orphans.
    pub app: Option<&'a App>,
    pub plan: Plan,
}

fn aborted() -> anyhow::Result<ExitCode> {
    writeln!(anstream::stderr().lock(), "aborted")?;
    Ok(ExitCode::from(4))
}

/// Preview, running-app check, confirmation, execution and report: steps
/// 4 to 7 of uninstall, shared with `orphans --clean`.
pub fn apply(
    env: &Env,
    cx: &Context,
    command: &str,
    targets: &[Target],
    act: &ActFlags,
) -> anyhow::Result<ExitCode> {
    let mut errors: Vec<ErrorEntry> = cx.inv.errors.iter().map(ErrorEntry::from).collect();
    errors.extend(cx.notes.iter().cloned());
    let plans: Vec<&Plan> = targets.iter().map(|t| &t.plan).collect();
    let empty = plans.iter().all(|p| p.actions.is_empty());

    if !act.json {
        let headed: Vec<(String, &Plan)> = targets
            .iter()
            .map(|t| (t.heading.clone(), &t.plan))
            .collect();
        render::preview(env, &headed, act.permanent)?;
        let mut err = anstream::stderr().lock();
        for e in &cx.inv.errors {
            writeln!(err, "warning: {}: {}", e.source.as_str(), e.message)?;
        }
        drop(err);
        print_notes(&cx.notes)?;
    }
    if act.dry_run || empty {
        if act.json {
            print_json_entries(command, &json!({ "plans": plans, "report": null }), &errors)?;
        } else if empty {
            writeln!(anstream::stdout().lock(), "nothing to do")?;
        }
        return Ok(ExitCode::SUCCESS);
    }

    let interactive = prompt::is_interactive();
    for app in targets.iter().filter_map(|t| t.app) {
        let pids = running_processes(env, app);
        if pids.is_empty() {
            continue;
        }
        let list: Vec<String> = pids.iter().map(u32::to_string).collect();
        let msg = format!("{} is running (pid {})", app.display_name, list.join(", "));
        writeln!(anstream::stderr().lock(), "{msg}")?;
        if act.force {
            continue;
        }
        if !interactive {
            return fail(1, "close it first or re-run with --force");
        }
        if !prompt::confirm(&format!(
            "{} is running; continue anyway?",
            app.display_name
        ))? {
            return aborted();
        }
    }
    if !act.yes {
        if !interactive {
            return fail(2, NON_INTERACTIVE);
        }
        if !prompt::confirm("Proceed?")? {
            return aborted();
        }
    }

    let reports: Vec<_> = plans
        .iter()
        .filter(|p| !p.actions.is_empty())
        .map(|p| execute(env, &cx.cfg, p))
        .collect();
    let all_ok = reports.iter().all(|r| r.results.iter().all(|x| x.ok));
    if act.json {
        print_json_entries(
            command,
            &json!({ "plans": plans, "report": reports }),
            &errors,
        )?;
    } else {
        writeln!(anstream::stdout().lock())?;
        render::report(env, &reports)?;
    }
    Ok(ExitCode::from(if all_ok { 0 } else { 3 }))
}
