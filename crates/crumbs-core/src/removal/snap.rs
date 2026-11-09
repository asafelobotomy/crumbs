//! Removing snaps: `snap remove --purge` as root. Data under `~/snap/<name>`
//! is a normal, restorable leftover.

use crate::command::{CommandRunner, resolve_trusted};
use crate::env::Env;
use crate::model::App;
use crate::plan::Action;

use super::{RemovalError, RemovalPlan, invalid, valid_name};

pub(super) fn plan(app: &App) -> Result<RemovalPlan, RemovalError> {
    let name = &app.name;
    if !valid_name(name) {
        return Err(invalid("snap", name));
    }
    Ok(RemovalPlan {
        actions: vec![Action::PrivilegedCommand {
            program: "snap".into(),
            args: vec!["remove".into(), "--purge".into(), name.clone()],
        }],
        ..RemovalPlan::default()
    })
}

/// The snaps of `names` that `snap list <name>` still shows (it exits
/// non-zero for a snap that is not installed). A missing `snap` is an error.
pub fn snap_still_installed(
    env: &Env,
    runner: &dyn CommandRunner,
    names: &[String],
) -> Result<Vec<String>, String> {
    let program = resolve_trusted(env, "snap").ok_or("snap is not installed")?;
    let mut still = Vec::new();
    for n in names {
        let out = runner
            .run(&program, &["list", n.as_str()])
            .map_err(|e| e.to_string())?;
        if out.status == 0 {
            still.push(n.clone());
        }
    }
    Ok(still)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::CmdOutput;
    use crate::model::{App, SourceKind};
    use crate::plan::Action;
    use crate::testing::{FakeRunner, FixtureTree};

    fn out(status: i32, stdout: &str) -> CmdOutput {
        CmdOutput {
            status,
            stdout: stdout.into(),
            stderr: String::new(),
            timed_out: false,
        }
    }

    #[test]
    fn snap_removal_is_purge_privileged() {
        let app = App::new(SourceKind::Snap, "spotify");
        let p = plan(&app).unwrap();
        assert_eq!(
            p.actions,
            [Action::PrivilegedCommand {
                program: "snap".into(),
                args: vec!["remove".into(), "--purge".into(), "spotify".into()],
            }]
        );
    }

    #[test]
    fn snap_name_validated() {
        let app = App::new(SourceKind::Snap, "-rf");
        assert!(matches!(plan(&app), Err(RemovalError::InvalidName(_))));
        let app = App::new(SourceKind::Snap, "a;b");
        assert!(matches!(plan(&app), Err(RemovalError::InvalidName(_))));
    }

    #[test]
    fn snap_post_check_gone_when_list_fails() {
        let t = FixtureTree::new();
        t.write_exec("/usr/bin/snap", b"#!/bin/sh\n");
        let r = FakeRunner::new()
            .on("snap", &["list", "gone"], out(1, ""))
            .on(
                "snap",
                &["list", "kept"],
                out(0, "Name Version\nkept 1.0\n"),
            );
        let names = vec!["gone".to_string(), "kept".to_string()];
        assert_eq!(
            snap_still_installed(&t.env(), &r, &names).unwrap(),
            ["kept"]
        );
    }

    #[test]
    fn snap_post_check_without_snap_is_an_error() {
        let r = FakeRunner::new();
        assert!(snap_still_installed(&FixtureTree::new().env(), &r, &["x".into()]).is_err());
    }
}
