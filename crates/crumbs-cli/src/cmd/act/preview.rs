//! The preview shown before acting, and the dry-run data.

use super::*;

pub(super) fn headed(targets: &[Target]) -> Vec<(String, &Plan)> {
    targets
        .iter()
        .map(|t| (t.heading.clone(), &t.plan))
        .collect()
}

/// The `note:` lines for `after_preview`, sanitised.
pub(super) fn after_text(after_preview: &[String]) -> String {
    after_preview
        .iter()
        .map(|note| format!("note: {}\n", render::sanitise(note)))
        .collect()
}

/// The human preview without colour: every plan, the `after_preview`
/// notes, then the warnings in `errors`, as the CLI shows them (it prints
/// the warnings on stderr).
#[cfg_attr(
    not(feature = "mcp"),
    allow(dead_code, reason = "only the mcp server calls it")
)]
pub fn preview_text(
    env: &Env,
    targets: &[Target],
    after_preview: &[String],
    errors: &[ErrorEntry],
) -> String {
    let mut text = String::new();
    // Writing to a String cannot fail.
    let _ = render::preview_to(&mut text, env, &headed(targets), false);
    text.push_str(&after_text(after_preview));
    text.push_str(&notes_text(errors));
    text
}

/// Prints the human preview: the plans and notes on stdout, the warnings
/// on stderr. Never prompts.
pub(super) fn print_preview(
    env: &Env,
    targets: &[Target],
    after_preview: &[String],
    errors: &[ErrorEntry],
) -> anyhow::Result<()> {
    render::preview(env, &headed(targets))?;
    anstream::stdout()
        .lock()
        .write_all(after_text(after_preview).as_bytes())?;
    Ok(print_notes(errors)?)
}

/// Each target app that is running, with `<name> is running (pid …)`.
pub(super) fn running_apps<'t>(env: &Env, targets: &'t [Target]) -> Vec<(&'t App, String)> {
    targets
        .iter()
        .filter_map(|t| t.app.as_ref())
        .filter_map(|app| {
            let pids = running_processes(env, app);
            if pids.is_empty() {
                return None;
            }
            let list: Vec<String> = pids.iter().map(u32::to_string).collect();
            let msg = format!(
                "{} is running (pid {})",
                render::sanitise(&app.display_name),
                list.join(", ")
            );
            Some((app, msg))
        })
        .collect()
}

/// How long `--close-running` waits for an app to quit after asking it to.
const QUIT_WAIT: std::time::Duration = std::time::Duration::from_secs(10);

/// `--close-running`: asks every running target app to quit with SIGTERM (never SIGKILL) and
/// waits up to [`QUIT_WAIT`] for it to go. An app that stays up is left to the usual
/// "is running" refusal.
pub(super) fn close_running_apps(env: &Env, targets: &[Target]) {
    let apps: Vec<&App> = targets.iter().filter_map(|t| t.app.as_ref()).collect();
    let mut any = false;
    for app in &apps {
        for pid in running_processes(env, app) {
            any |= crumbs_core::executor::ask_to_quit(pid);
        }
    }
    if !any {
        return;
    }
    let deadline = std::time::Instant::now() + QUIT_WAIT;
    while std::time::Instant::now() < deadline {
        if apps
            .iter()
            .all(|app| running_processes(env, app).is_empty())
        {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
}

/// The `--dry-run --json` data: every plan, and no report.
pub fn dry_run_data(targets: &[Target]) -> serde_json::Value {
    let plans: Vec<&Plan> = targets.iter().map(|t| &t.plan).collect();
    json!({ "plans": plans, "report": null })
}
