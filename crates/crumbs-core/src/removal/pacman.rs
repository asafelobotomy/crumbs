use std::fs;

use crate::command::{CommandRunner, resolve_trusted};
use crate::env::Env;
use crate::model::{App, SourceKind};
use crate::plan::Action;
use crate::sources::pacman::{first, parse_sections};

use super::{MSG_BUSY, RemovalError, RemovalPlan, invalid, lock_path, valid_name};

const DB_DIR: &str = "/var/lib/pacman/local";

pub(super) fn plan(
    env: &Env,
    runner: &dyn CommandRunner,
    app: &App,
) -> Result<RemovalPlan, RemovalError> {
    if !valid_name(&app.name) {
        return Err(invalid("package name", &app.name));
    }
    if lock_path(env).exists() {
        return Err(RemovalError::Busy(MSG_BUSY.into()));
    }
    let program = resolve_trusted(env, "pacman")
        .ok_or_else(|| RemovalError::Unsupported("pacman is not installed".into()))?;
    // pacman refuses --print together with -n, so simulate with -Rs.
    let out = runner
        .run(
            &program,
            &["-Rs", "--print", "--print-format", "%n %v", &app.name],
        )
        .map_err(|e| RemovalError::Refused(e.to_string()))?;
    if out.status != 0 {
        let text = out.stderr.trim();
        return Err(RemovalError::Refused(if text.is_empty() {
            format!("pacman refused the removal (status {})", out.status)
        } else {
            text.to_string()
        }));
    }

    let mut names = vec![app.name.clone()];
    let mut versions = vec![app.version.clone().unwrap_or_default()];
    let mut also_removed = Vec::new();
    for line in out.stdout.lines().filter(|l| !l.trim().is_empty()) {
        let mut parts = line.split_whitespace();
        let (Some(name), Some(version), None) = (parts.next(), parts.next(), parts.next()) else {
            return Err(invalid("simulation line", line));
        };
        if !valid_name(name) {
            return Err(invalid("package name", name));
        }
        if name == app.name {
            // The app's own version is the one pacman will remove.
            versions[0] = version.to_string();
        } else if !names.iter().any(|n| n == name) {
            names.push(name.to_string());
            versions.push(version.to_string());
            also_removed.push((name.to_string(), version.to_string()));
        }
    }

    let mut args = vec!["-Rns".to_string(), "--noconfirm".to_string()];
    args.extend(names.iter().cloned());
    Ok(RemovalPlan {
        actions: vec![Action::PackageRemove {
            source: SourceKind::Pacman,
            names,
            versions,
            program: "pacman".into(),
            args,
        }],
        also_removed,
        notes: Vec::new(),
    })
}

/// Re-reads the local database: the entries of `names` whose package is
/// still there (matched on `%NAME%`, as the source does).
pub fn pacman_still_installed(env: &Env, names: &[String]) -> Vec<String> {
    let installed: Vec<String> = fs::read_dir(env.sys(DB_DIR))
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().is_dir())
        .filter_map(|e| fs::read(e.path().join("desc")).ok())
        .filter_map(|b| first(&parse_sections(&String::from_utf8_lossy(&b)), "NAME"))
        .collect();
    names
        .iter()
        .filter(|n| installed.contains(n))
        .cloned()
        .collect()
}
