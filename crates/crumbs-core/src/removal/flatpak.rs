use crate::command::{CommandRunner, resolve_trusted};
use crate::env::Env;
use crate::model::{App, Installation};
use crate::plan::Action;

use super::{RemovalError, RemovalPlan, invalid, valid_name};

pub(super) fn plan(app: &App) -> Result<RemovalPlan, RemovalError> {
    let id = &app.name;
    if !valid_name(id) {
        return Err(invalid("Flatpak id", id));
    }
    let tail = |flag: String| {
        vec![
            "uninstall".to_string(),
            flag,
            "-y".into(),
            "--noninteractive".into(),
            id.clone(),
        ]
    };
    let program = "flatpak".to_string();
    // `--delete-data` is never passed: ~/.var/app/<id> is trashed as a
    // restorable leftover instead.
    let action = match &app.installation {
        Installation::User => Action::UserCommand {
            program,
            args: tail("--user".into()),
        },
        Installation::System => Action::PrivilegedCommand {
            program,
            args: tail("--system".into()),
        },
        Installation::Custom(name) => {
            if !valid_name(name) {
                return Err(invalid("Flatpak installation", name));
            }
            Action::PrivilegedCommand {
                program,
                args: tail(format!("--installation={name}")),
            }
        }
    };
    Ok(RemovalPlan {
        actions: vec![action],
        ..RemovalPlan::default()
    })
}

/// The app ids of `names` that `flatpak list` still shows.
pub fn flatpak_still_installed(
    env: &Env,
    runner: &dyn CommandRunner,
    names: &[String],
) -> Vec<String> {
    let Some(program) = resolve_trusted(env, "flatpak") else {
        return Vec::new();
    };
    let Ok(out) = runner.run(&program, &["list", "--app", "--columns=application"]) else {
        return Vec::new();
    };
    if out.status != 0 {
        return Vec::new();
    }
    names
        .iter()
        .filter(|n| out.stdout.lines().any(|l| l.trim() == n.as_str()))
        .cloned()
        .collect()
}
