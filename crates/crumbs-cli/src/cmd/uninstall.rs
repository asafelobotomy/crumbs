//! `uninstall`: resolve apps exactly, plan their removal, then the shared
//! confirm-and-execute flow in [`super::act`].

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::Args;
use crumbs_core::desktop::parse_desktop;
use crumbs_core::inventory::{Inventory, Resolved, find_app, find_app_exact};
use crumbs_core::leftovers::{Finding, FindingKind, find_leftovers};
use crumbs_core::plan::{PlanOptions, build_plan};
use crumbs_core::safety;
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
        keep_data: args.keep_data,
        leftovers_only: args.leftovers_only,
    };
    let mut targets = Vec::new();
    for app in apps {
        let mut found = plannable(find_leftovers(&cx.idx, app, args.show_low));
        if args.leftovers_only {
            found.retain(|f| !is_own_launcher(env, app, f));
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
    use crumbs_core::safety::Fingerprint;
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
            fingerprint: Fingerprint {
                dev: 0,
                ino: 0,
                is_symlink: false,
            },
            mime_lines: Vec::new(),
        }
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
