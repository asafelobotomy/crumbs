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
    // Any directory entry counts, even a dangling symlink.
    if fs::symlink_metadata(lock_path(env)).is_ok() {
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
        // pacman prints the reason (`:: removing X breaks dependency ...`)
        // on stdout and the summary on stderr.
        let mut lines: Vec<&str> = Some(out.stderr.trim())
            .filter(|l| !l.is_empty())
            .into_iter()
            .collect();
        lines.extend(
            out.stdout
                .lines()
                .map(str::trim)
                .filter(|l| l.starts_with("::")),
        );
        return Err(RemovalError::Refused(if lines.is_empty() {
            format!("pacman refused the removal (status {})", out.status)
        } else {
            lines.join("\n")
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

    // No -s: the full simulated list is passed, so pacman can never remove
    // more than previewed (and refuses if the database changed).
    let mut args = vec!["-Rn".to_string(), "--noconfirm".to_string()];
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
/// still there (matched on `%NAME%`, as the source does). Any unreadable
/// part of the database is an error.
pub fn pacman_still_installed(env: &Env, names: &[String]) -> Result<Vec<String>, String> {
    let db = env.sys(DB_DIR);
    let mut installed: Vec<String> = Vec::new();
    for entry in fs::read_dir(&db).map_err(|e| format!("cannot read {DB_DIR}: {e}"))? {
        let path = entry
            .map_err(|e| format!("cannot read {DB_DIR}: {e}"))?
            .path();
        if !path.is_dir() {
            continue;
        }
        let desc = path.join("desc");
        let bytes = fs::read(&desc)
            .map_err(|e| format!("cannot read {}: {e}", env.unsys(&desc).display()))?;
        let name = first(&parse_sections(&String::from_utf8_lossy(&bytes)), "NAME")
            .ok_or_else(|| format!("{} has no %NAME%", env.unsys(&desc).display()))?;
        installed.push(name);
    }
    Ok(names
        .iter()
        .filter(|n| installed.contains(n))
        .cloned()
        .collect())
}
