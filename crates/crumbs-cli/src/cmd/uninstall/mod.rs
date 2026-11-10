//! `uninstall`: resolve apps exactly, plan their removal, then the shared
//! confirm-and-execute flow in [`super::act`].

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::Args;
use crumbs_core::command::SystemRunner;
use crumbs_core::desktop::parse_desktop;
use crumbs_core::inventory::{Inventory, Resolved, find_app, find_app_exact};
use crumbs_core::leftovers::{Finding, FindingKind, find_leftovers, find_system_leftovers};
use crumbs_core::plan::{
    NOTE_APP_STAYS, Plan, PlanOptions, PlannedAction, build_plan_unmanaged, is_system_leftover,
};
use crumbs_core::removal::{
    RemovalError, RemovalPlan, UnmanagedTarget, plan_removal, plan_unmanaged,
};
use crumbs_core::safety::{self, Fingerprint, Scope};
use crumbs_core::{App, Env, SourceKind};

use super::act::{
    ActFlags, Out, Target, apply, choose, default_selection, drop_overlaps, plannable, precheck,
    refuse_source_errors, stop,
};
use super::build::{Built, Draft, Pick, Refusal, Stop, select_findings};
use super::context::Context;
use super::render::sanitise;

mod keep;
use keep::*;
#[derive(Args, Debug)]
pub struct UninstallArgs {
    /// App ids, names, desktop ids or display names (exact matches only)
    #[arg(required = true)]
    pub apps: Vec<String>,
    /// Keep the app itself; remove only its leftovers
    #[arg(long)]
    pub leftovers_only: bool,
    /// Keep data and settings; remove only caches (and, with the AppImage
    /// file, its own launchers and icons)
    #[arg(long)]
    pub keep_data: bool,
    /// Include low-confidence leftovers
    #[arg(long)]
    pub show_low: bool,
    /// Allow removing a package that is part of the uQ desktop (uq, scdr,
    /// noti-fi, crum-bs, ...) without asking a second time
    #[arg(long)]
    pub desktop: bool,
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
    let pick = if args.act.interactive {
        Pick::Checklist
    } else {
        Pick::Defaults
    };
    let Draft { built, cfg, .. } = match build(env, args, pick) {
        Ok(draft) => draft,
        Err(s) => return stop(out, s),
    };
    apply(
        env,
        &cfg,
        &built.errors,
        out,
        &built.targets,
        &args.act,
        &built.after_preview,
    )
}

/// Plans the removal of `args.apps` and the leftovers `pick` chooses.
pub(crate) fn build(env: &Env, args: &UninstallArgs, pick: Pick<PathBuf>) -> Result<Draft, Stop> {
    let cx = Context::load(env)?;
    let mut apps: Vec<&App> = Vec::new();
    for query in &args.apps {
        let app = resolve(&cx.inv, query)?;
        if !apps.iter().any(|a| a.id == app.id) {
            apps.push(app);
        }
    }
    if !args.leftovers_only {
        for app in &apps {
            confirm_desktop_removal(app, args)?;
        }
    }
    // How each app itself goes, in `apps` order; none with --leftovers-only.
    // An unmanaged app goes by its path (`plan_unmanaged`), the rest by
    // their package manager.
    let mut removals: Vec<Option<RemovalPlan>> = Vec::new();
    let mut unmanaged: Vec<Option<UnmanagedTarget>> = Vec::new();
    for app in &apps {
        let (mut removal, mut target) = (None, None);
        if !args.leftovers_only {
            let planned = if app.source == SourceKind::Unmanaged {
                plan_unmanaged(env, &cx.cfg, &cx.inv, app).map(|t| target = Some(t))
            } else {
                plan_removal(env, &SystemRunner::new(), app).map(|r| removal = Some(r))
            };
            if let Err(e) = planned {
                return Err(removal_refusal(app, &e).into());
            }
        }
        removals.push(removal);
        unmanaged.push(target);
    }
    if !args.leftovers_only {
        for app in &apps {
            if let Err(reason) = appimage_removable(env, &cx, app) {
                let msg = format!(
                    "cannot remove {}: {reason}; use --leftovers-only to remove only its leftovers",
                    app.id
                );
                return Err(Refusal::new(1, msg).into());
            }
        }
    }
    refuse_source_errors(&cx.inv, &args.act)?;
    let opts = PlanOptions {
        permanent: args.act.permanent,
        // `--keep-data` is applied to the findings below, before choosing.
        keep_data: false,
        leftovers_only: args.leftovers_only,
    };
    let found: Vec<Vec<Finding>> = apps
        .iter()
        .map(|app| {
            let mut found = plannable(find_leftovers(&cx.idx, app, args.show_low));
            // Offered, never pre-selected: root deletes only on an explicit pick.
            found.extend(find_system_leftovers(
                env,
                &cx.cfg,
                &cx.idx,
                app,
                args.show_low,
            ));
            found.retain(|f| keeps(env, app, f, args));
            found
        })
        .collect();
    let selectable: Vec<Finding> = found.iter().flatten().cloned().collect();
    let wanted = match pick {
        Pick::Named(select) => select_findings(env, &selectable, select)?,
        Pick::Defaults | Pick::Checklist => Vec::new(),
    };
    let mut targets = Vec::new();
    let mut unselected_system = 0usize;
    for (((app, removal), target), found) in
        apps.into_iter().zip(removals).zip(unmanaged).zip(found)
    {
        let system_found: Vec<PathBuf> = found
            .iter()
            .filter(|f| f.scope == Scope::SystemTree)
            .map(|f| f.path.clone())
            .collect();
        let chosen = match pick {
            Pick::Defaults => default_selection(found, |f| f.preselected),
            Pick::Named(_) => found
                .into_iter()
                .filter(|f| wanted.contains(&f.path))
                .collect(),
            Pick::Checklist => {
                let prompt_text = format!("Leftovers of {} to remove", sanitise(&app.display_name));
                choose(env, &prompt_text, found, true, |f| f.preselected)?.ok_or(Stop::Aborted)?
            }
        };
        let chosen_paths: Vec<PathBuf> = chosen.iter().map(|f| f.path.clone()).collect();
        let mut plan = build_plan_unmanaged(env, Some(app), target.as_ref(), &chosen, &opts)?;
        if let Some(removal) = removal {
            attach_removal(&mut plan, removal);
        }
        // Offered and not picked, and not removed with the app either.
        unselected_system += system_found
            .iter()
            .filter(|p| !chosen_paths.contains(p))
            .filter(|p| {
                !plan
                    .actions
                    .iter()
                    .any(|a| a.action.path().is_some_and(|ap| p.starts_with(ap)))
            })
            .count();
        targets.push(Target {
            heading: format!("{}  {}", sanitise(&app.display_name), sanitise(&app.id)),
            app: Some(app.clone()),
            plan,
        });
    }
    drop_overlaps(&mut targets);
    let built = Built {
        targets,
        errors: cx.errors(),
        after_preview: unselected_note(unselected_system),
        cli_command: cli_command(args, &pick),
    };
    Ok(Draft {
        built,
        cfg: cx.cfg,
        held: Vec::new(),
    })
}

/// A package of the uQ desktop goes only when asked for twice: `--desktop`,
/// or a yes to this question at a terminal before the usual "Proceed?". A
/// dry run at a terminal just previews; without a terminal (scripts, the
/// MCP server) or with `--yes`, it is refused without `--desktop`.
fn confirm_desktop_removal(app: &App, args: &UninstallArgs) -> Result<(), Stop> {
    let Some(desktop) = app.part_of.as_deref() else {
        return Ok(());
    };
    if args.desktop {
        return Ok(());
    }
    let interactive = crate::prompt::is_interactive();
    if interactive && args.act.dry_run {
        return Ok(());
    }
    if interactive && !args.act.yes {
        let question = format!(
            "{} is part of the {desktop} desktop; removing it can leave your session without it. \
             Remove it anyway?",
            sanitise(&app.display_name)
        );
        return match crate::prompt::confirm(&question) {
            Ok(true) => Ok(()),
            Ok(false) => Err(Stop::Aborted),
            Err(e) => Err(Stop::Failed(e)),
        };
    }
    Err(Refusal::new(
        1,
        format!(
            "refusing: {} is part of the {desktop} desktop; re-run with --desktop to remove it, \
             or --leftovers-only to keep it",
            app.id
        ),
    )
    .into())
}

/// The CLI command that plans the same: a picked selection is `-i`.
fn cli_command(args: &UninstallArgs, pick: &Pick<PathBuf>) -> Vec<String> {
    let mut argv = vec!["crumbs".to_string(), "uninstall".to_string()];
    argv.extend(args.apps.iter().cloned());
    let flags = [
        (args.leftovers_only, "--leftovers-only"),
        (args.keep_data, "--keep-data"),
        (args.show_low, "--show-low"),
        (args.desktop, "--desktop"),
        (args.act.permanent, "--permanent"),
        (args.act.force, "--force"),
        (!matches!(pick, Pick::Defaults), "-i"),
    ];
    argv.extend(
        flags
            .iter()
            .filter(|(on, _)| *on)
            .map(|(_, f)| f.to_string()),
    );
    argv
}

/// The preview note for system leftovers that were offered but not picked.
fn unselected_note(n: usize) -> Vec<String> {
    match n {
        0 => Vec::new(),
        1 => vec!["1 system leftover not selected (pick with -i)".into()],
        n => vec![format!("{n} system leftovers not selected (pick with -i)")],
    }
}

pub const NOTE_NO_UNDO: &str = "this cannot be undone by crumbs";
const REASON_APP: &str = "the app itself";

/// Puts the actions that remove the app itself first in `plan`, each
/// depending on the one before; every leftover then depends on the last of
/// them, so nothing of the app's goes unless all of the app did. A system
/// leftover depends on nothing: it was picked by hand and no package owns
/// it (as with `--leftovers-only`), and the root batch cannot wait. Nothing
/// changes when there are none (an AppImage's file is already in the
/// plan).
fn attach_removal(plan: &mut Plan, removal: RemovalPlan) {
    let n = removal.actions.len();
    if n == 0 {
        return;
    }
    plan.notes.retain(|note| note != NOTE_APP_STAYS);
    for pa in &mut plan.actions {
        if is_system_leftover(pa) {
            continue;
        }
        pa.depends_on = Some(pa.depends_on.map_or(n - 1, |d| d + n));
    }
    let first: Vec<PlannedAction> = removal
        .actions
        .into_iter()
        .enumerate()
        .map(|(k, action)| PlannedAction {
            action,
            size: 0,
            fingerprint: Fingerprint {
                dev: 0,
                ino: 0,
                is_symlink: false,
                ctime: 0,
                ctime_nsec: 0,
            },
            scope: Scope::Home,
            depends_on: k.checked_sub(1),
            depends_on_plan: None,
            size_known: true,
            reason: REASON_APP.into(),
        })
        .collect();
    plan.actions.splice(0..0, first);
    plan.notes.extend(removal.notes);
    plan.notes.push(NOTE_NO_UNDO.into());
}

/// Why the app itself cannot be removed: exit 2 when crumbs does not
/// support it, else 1. Text from the package manager may span lines; the
/// first goes into the error line, the rest below it.
fn removal_refusal(app: &App, e: &RemovalError) -> Refusal {
    match e {
        RemovalError::Unsupported(msg) => Refusal::new(2, msg.clone()),
        RemovalError::Busy(msg) => Refusal::new(1, msg.clone()),
        RemovalError::Refused(text) | RemovalError::InvalidName(text) => {
            let mut lines = text.lines().map(str::trim).filter(|l| !l.is_empty());
            // pacman starts its summary with its own `error: `.
            let first = lines
                .next()
                .map_or("refused", |l| l.strip_prefix("error: ").unwrap_or(l));
            let rest: Vec<String> = lines.map(str::to_string).collect();
            Refusal::new(1, format!("cannot remove {}: {first}", app.id)).with_details(rest)
        }
    }
}

#[cfg(test)]
mod tests;
