//! `uninstall`: resolve apps exactly, plan their removal, then the shared
//! confirm-and-execute flow in [`super::act`].

use std::process::ExitCode;

use clap::Args;
use crumbs_core::inventory::{Inventory, Resolved, find_app, find_app_exact};
use crumbs_core::leftovers::{Finding, FindingKind, find_leftovers};
use crumbs_core::plan::{PlanOptions, build_plan};
use crumbs_core::{App, Env, SourceKind};

use super::act::{
    ActFlags, Out, Target, apply, choose, drop_overlaps, plannable, precheck, refuse_source_errors,
};
use super::context::Context;

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

pub fn run(env: &Env, args: &UninstallArgs) -> anyhow::Result<ExitCode> {
    let out = Out {
        command: "uninstall",
        json: args.act.json,
    };
    if let Some(code) = precheck(out, &args.act)? {
        return Ok(code);
    }
    let cx = Context::load(env)?;
    let mut apps: Vec<&App> = Vec::new();
    for query in &args.apps {
        match resolve(out, &cx.inv, query)? {
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
        return out.fail(2, &msg);
    }
    if let Some(code) = refuse_source_errors(out, &cx.inv, &args.act)? {
        return Ok(code);
    }
    let opts = PlanOptions {
        permanent: args.act.permanent,
        keep_data: args.keep_data,
        leftovers_only: args.leftovers_only,
    };
    let mut targets = Vec::new();
    for app in apps {
        let mut found = plannable(find_leftovers(&cx.idx, app, args.show_low));
        if args.leftovers_only {
            found.retain(|f| !is_own_launcher(app, f));
        }
        let prompt_text = format!("Leftovers of {} to remove", app.display_name);
        let Some(chosen) = choose(env, &prompt_text, found, args.act.interactive, |f| {
            f.preselected
        })?
        else {
            return out.aborted();
        };
        targets.push(Target {
            heading: format!("{}  {}", app.display_name, app.id),
            app: Some(app),
            plan: build_plan(env, Some(app), &chosen, &opts)?,
        });
    }
    drop_overlaps(&mut targets);
    apply(env, &cx, out, &targets, &args.act)
}

/// A launcher of the app itself (named by one of its desktop IDs), or an
/// icon named after one. They stay while the app stays; autostart entries
/// do not count.
fn is_own_launcher(app: &App, f: &Finding) -> bool {
    match f.kind {
        FindingKind::DesktopEntry => f
            .path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| app.desktop_ids.iter().any(|d| d == n)),
        FindingKind::Icon => f
            .path
            .file_stem()
            .and_then(|s| s.to_str())
            .is_some_and(|s| {
                app.desktop_ids
                    .iter()
                    .any(|d| d.strip_suffix(".desktop").unwrap_or(d) == s)
            }),
        _ => false,
    }
}

/// The app for one query (exact matches only), or the exit code after
/// explaining why not.
fn resolve<'a>(
    out: Out,
    inv: &'a Inventory,
    query: &str,
) -> anyhow::Result<Result<&'a App, ExitCode>> {
    let list = |c: Vec<&App>| -> Vec<String> {
        c.iter()
            .map(|a| format!("{}  {}  {}", a.id, a.display_name, a.source.as_str()))
            .collect()
    };
    let code = match find_app_exact(inv, query) {
        Resolved::Found(app) => return Ok(Ok(app)),
        Resolved::Ambiguous(c) => {
            out.fail_with(2, &format!("'{query}' matches several apps:"), &list(c))?
        }
        Resolved::NotFound => match find_app(inv, query) {
            Resolved::Found(app) => out.fail(
                2,
                &format!("no app is exactly '{query}'; did you mean '{}'?", app.id),
            )?,
            Resolved::Ambiguous(c) => out.fail_with(
                2,
                &format!("no app is exactly '{query}'; did you mean one of these?"),
                &list(c),
            )?,
            Resolved::NotFound => out.fail(2, &format!("no installed app matches '{query}'"))?,
        },
    };
    Ok(Err(code))
}
