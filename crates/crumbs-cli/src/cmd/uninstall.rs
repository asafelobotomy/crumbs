//! `uninstall`: resolve apps exactly, plan their removal, then the shared
//! confirm-and-execute flow in [`super::act`].

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::Args;
use crumbs_core::command::SystemRunner;
use crumbs_core::desktop::parse_desktop;
use crumbs_core::inventory::{Inventory, Resolved, find_app, find_app_exact};
use crumbs_core::leftovers::{Finding, FindingKind, find_leftovers, find_system_leftovers};
use crumbs_core::plan::{NOTE_APP_STAYS, Plan, PlanOptions, PlannedAction, build_plan_unmanaged};
use crumbs_core::removal::{
    RemovalError, RemovalPlan, UnmanagedTarget, plan_removal, plan_unmanaged,
};
use crumbs_core::safety::{self, Fingerprint, Scope};
use crumbs_core::{App, Env, SourceKind};

use super::act::{
    ActFlags, Out, Target, apply, choose, drop_overlaps, plannable, precheck, refuse_source_errors,
};
use super::context::Context;
use super::render::sanitise;

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
                return refuse_removal(out, app, &e);
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
                return out.fail(1, &msg);
            }
        }
    }
    if let Some(code) = refuse_source_errors(out, &cx.inv, &args.act)? {
        return Ok(code);
    }
    let opts = PlanOptions {
        permanent: args.act.permanent,
        // `--keep-data` is applied to the findings below, before choosing.
        keep_data: false,
        leftovers_only: args.leftovers_only,
    };
    let mut targets = Vec::new();
    for ((app, removal), target) in apps.into_iter().zip(removals).zip(unmanaged) {
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
        let prompt_text = format!("Leftovers of {} to remove", sanitise(&app.display_name));
        let Some(chosen) = choose(env, &prompt_text, found, args.act.interactive, |f| {
            f.preselected
        })?
        else {
            return out.aborted();
        };
        let mut plan = build_plan_unmanaged(env, Some(app), target.as_ref(), &chosen, &opts)?;
        if let Some(removal) = removal {
            attach_removal(&mut plan, removal);
        }
        targets.push(Target {
            heading: format!("{}  {}", sanitise(&app.display_name), sanitise(&app.id)),
            app: Some(app),
            plan,
        });
    }
    drop_overlaps(&mut targets);
    apply(env, &cx.cfg, &cx.errors(), out, &targets, &args.act)
}

pub const NOTE_NO_UNDO: &str = "this cannot be undone by crumbs";
const REASON_APP: &str = "the app itself";

/// Puts the actions that remove the app itself first in `plan`, each
/// depending on the one before; every leftover then depends on the last of
/// them, so nothing of the app's goes unless all of the app did. Nothing
/// changes when there are none (an AppImage's file is already in the
/// plan).
fn attach_removal(plan: &mut Plan, removal: RemovalPlan) {
    let n = removal.actions.len();
    if n == 0 {
        return;
    }
    plan.notes.retain(|note| note != NOTE_APP_STAYS);
    for pa in &mut plan.actions {
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
            reason: REASON_APP.into(),
        })
        .collect();
    plan.actions.splice(0..0, first);
    plan.notes.extend(removal.notes);
    plan.notes.push(NOTE_NO_UNDO.into());
}

/// Explains why the app itself cannot be removed: exit 2 when crumbs does
/// not support it, else 1. Text from the package manager may span lines;
/// the first goes into the error line, the rest below it (sanitised).
fn refuse_removal(out: Out, app: &App, e: &RemovalError) -> anyhow::Result<ExitCode> {
    match e {
        RemovalError::Unsupported(msg) => out.fail(2, msg),
        RemovalError::Busy(msg) => out.fail(1, msg),
        RemovalError::Refused(text) | RemovalError::InvalidName(text) => {
            let mut lines = text.lines().map(str::trim).filter(|l| !l.is_empty());
            // pacman starts its summary with its own `error: `.
            let first = lines
                .next()
                .map_or("refused", |l| l.strip_prefix("error: ").unwrap_or(l));
            let rest: Vec<String> = lines.map(str::to_string).collect();
            out.fail_with(1, &format!("cannot remove {}: {first}", app.id), &rest)
        }
    }
}

/// Whether a finding may be planned. The launchers and icons stay with an
/// app that stays (`--leftovers-only`). `--keep-data` keeps everything but
/// caches; when the AppImage file itself goes, its own launchers and icons
/// go with it so nothing is left pointing at a removed file.
fn keeps(env: &Env, app: &App, f: &Finding, args: &UninstallArgs) -> bool {
    if args.leftovers_only {
        return !is_own_launcher(env, app, f) && (!args.keep_data || f.kind == FindingKind::Cache);
    }
    if args.keep_data {
        return f.kind == FindingKind::Cache
            || (matches!(f.kind, FindingKind::DesktopEntry | FindingKind::Icon)
                && is_own_launcher(env, app, f));
    }
    true
}

/// Whether an AppImage's own file passes the same checks the executor runs
/// before trashing it. Checked before anything is previewed, so an app is
/// never half removed (leftovers gone, file refused).
fn appimage_removable(env: &Env, cx: &Context, app: &App) -> Result<(), String> {
    if app.source != SourceKind::AppImage {
        return Ok(());
    }
    let Some(location) = &app.location else {
        return Err("its file location is unknown".into());
    };
    safety::verify_now(env, &cx.cfg, safety::Scope::Home, location)
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// Part of how the app is launched or opened, so it stays while the app
/// stays: a launcher named by one of its desktop IDs or whose `Exec` runs
/// its file, an icon named after one of its desktop IDs, and its
/// file-type associations. Autostart entries do not count.
fn is_own_launcher(env: &Env, app: &App, f: &Finding) -> bool {
    match f.kind {
        FindingKind::MimeAssoc => true,
        FindingKind::DesktopEntry => {
            let Some(name) = f.path.file_name().and_then(|n| n.to_str()) else {
                return true;
            };
            app.desktop_ids.iter().any(|d| d == name) || runs_app_file(env, app, name, &f.path)
        }
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

/// Whether the launcher at `path` runs the app's own file (`Exec` names
/// its location, `~/` expanded). An unreadable launcher counts as running
/// it, so it is kept.
fn runs_app_file(env: &Env, app: &App, name: &str, path: &Path) -> bool {
    let Some(location) = &app.location else {
        return false;
    };
    let Ok(bytes) = std::fs::read(env.sys_path(path)) else {
        return true;
    };
    let entry = parse_desktop(name, path, &bytes);
    entry.exec_program.as_deref().is_some_and(|prog| {
        let exec = match prog.strip_prefix("~/") {
            Some(rest) => env.unsys(&env.home).join(rest),
            None => PathBuf::from(prog),
        };
        &exec == location
    })
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

#[cfg(test)]
mod tests {
    use super::*;
    use crumbs_core::leftovers::Confidence;
    use crumbs_core::plan::{Action, build_plan};
    use crumbs_core::safety::{Fingerprint, Scope};
    use crumbs_core::size::DiskUsage;
    use crumbs_core::testing::FixtureTree;

    fn finding(path: &str, kind: FindingKind) -> Finding {
        Finding {
            path: PathBuf::from(path),
            kind,
            confidence: Confidence::High,
            shared: false,
            reason: String::new(),
            size: DiskUsage::default(),
            preselected: true,
            scope: Scope::Home,
            fingerprint: Fingerprint {
                dev: 0,
                ino: 0,
                is_symlink: false,
                ctime: 0,
                ctime_nsec: 0,
            },
            mime_lines: Vec::new(),
        }
    }

    fn removal_plan(n: usize) -> RemovalPlan {
        RemovalPlan {
            actions: (0..n)
                .map(|k| Action::PrivilegedCommand {
                    program: "flatpak".into(),
                    args: vec![k.to_string()],
                })
                .collect(),
            ..RemovalPlan::default()
        }
    }

    #[test]
    fn leftovers_depend_on_every_removal_action() {
        let t = FixtureTree::new();
        let env = t.env();
        let app = App::new(SourceKind::Flatpak, "org.x.App");
        let mut f = finding("/home/user/.var/app/org.x.App", FindingKind::Data);
        f.size = DiskUsage::default();
        let mut plan =
            build_plan(&env, Some(&app), &[f.clone(), f], &PlanOptions::default()).unwrap();
        attach_removal(&mut plan, removal_plan(2));
        let deps: Vec<Option<usize>> = plan.actions.iter().map(|a| a.depends_on).collect();
        // The second removal waits for the first; leftovers for the last.
        assert_eq!(deps, [None, Some(0), Some(1), Some(1)]);
        assert!(!plan.notes.iter().any(|n| n == NOTE_APP_STAYS));
        assert!(plan.notes.iter().any(|n| n == NOTE_NO_UNDO));
    }

    #[test]
    fn launcher_running_the_app_file_is_its_own() {
        let t = FixtureTree::new();
        let env = t.env();
        t.desktop(
            "/home/user/.local/share/applications/runs-it.desktop",
            "[Desktop Entry]\nType=Application\nName=X\nExec=/home/user/Applications/X.AppImage %U\n",
        );
        t.desktop(
            "/home/user/.local/share/applications/tilde.desktop",
            "[Desktop Entry]\nType=Application\nName=X\nExec=~/Applications/X.AppImage\n",
        );
        t.desktop(
            "/home/user/.local/share/applications/other.desktop",
            "[Desktop Entry]\nType=Application\nName=X\nExec=/usr/bin/x\n",
        );
        let mut app = App::new(SourceKind::AppImage, "X.AppImage");
        app.location = Some(PathBuf::from("/home/user/Applications/X.AppImage"));
        assert!(app.desktop_ids.is_empty());
        let own = |rel: &str, kind| is_own_launcher(&env, &app, &finding(rel, kind));
        assert!(own(
            "/home/user/.local/share/applications/runs-it.desktop",
            FindingKind::DesktopEntry
        ));
        assert!(own(
            "/home/user/.local/share/applications/tilde.desktop",
            FindingKind::DesktopEntry
        ));
        assert!(!own(
            "/home/user/.local/share/applications/other.desktop",
            FindingKind::DesktopEntry
        ));
        assert!(own(
            "/home/user/.config/mimeapps.list",
            FindingKind::MimeAssoc
        ));
        assert!(!own(
            "/home/user/.config/autostart/runs-it.desktop",
            FindingKind::Autostart
        ));
    }
}
