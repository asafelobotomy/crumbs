//! Which findings an uninstall keeps, and resolving the app the user named.

use super::*;

/// Whether a finding may be planned. The launchers and icons stay with an
/// app that stays (`--leftovers-only`). `--keep-data` keeps everything but
/// caches; when the AppImage file itself goes, its own launchers and icons
/// go with it, and so do its file-type associations, so nothing is left
/// pointing at a removed file.
pub(super) fn keeps(env: &Env, app: &App, f: &Finding, args: &UninstallArgs) -> bool {
    if args.leftovers_only {
        return !is_own_launcher(env, app, f) && (!args.keep_data || f.kind == FindingKind::Cache);
    }
    if args.keep_data {
        return f.kind == FindingKind::Cache
            || (matches!(
                f.kind,
                FindingKind::DesktopEntry | FindingKind::Icon | FindingKind::MimeAssoc
            ) && is_own_launcher(env, app, f));
    }
    true
}

/// Whether an AppImage's own file passes the same checks the executor runs
/// before trashing it. Checked before anything is previewed, so an app is
/// never half removed (leftovers gone, file refused).
pub(super) fn appimage_removable(env: &Env, cx: &Context, app: &App) -> Result<(), String> {
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
pub(super) fn is_own_launcher(env: &Env, app: &App, f: &Finding) -> bool {
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
pub(super) fn runs_app_file(env: &Env, app: &App, name: &str, path: &Path) -> bool {
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

/// The app for one query (exact matches only), or why not.
pub(super) fn resolve<'a>(inv: &'a Inventory, query: &str) -> Result<&'a App, Refusal> {
    let list = |c: Vec<&App>| -> Vec<String> {
        c.iter()
            .map(|a| format!("{}  {}  {}", a.id, a.display_name, a.source.as_str()))
            .collect()
    };
    Err(match find_app_exact(inv, query) {
        Resolved::Found(app) => return Ok(app),
        Resolved::Ambiguous(c) => {
            Refusal::new(2, format!("'{query}' matches several apps:")).with_details(list(c))
        }
        Resolved::NotFound => match find_app(inv, query) {
            Resolved::Found(app) => Refusal::new(
                2,
                format!("no app is exactly '{query}'; did you mean '{}'?", app.id),
            ),
            Resolved::Ambiguous(c) => Refusal::new(
                2,
                format!("no app is exactly '{query}'; did you mean one of these?"),
            )
            .with_details(list(c)),
            Resolved::NotFound => Refusal::new(2, format!("no installed app matches '{query}'")),
        },
    })
}
