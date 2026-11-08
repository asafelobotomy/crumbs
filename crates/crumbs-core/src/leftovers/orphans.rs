//! Orphans: leftovers that no installed app claims.

use std::cmp::Reverse;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::command::TRUSTED_DIRS;
use crate::config::Config;
use crate::env::Env;
use crate::ownership::tmpfiles_declared;
use crate::rules::{Rule, RuleOrigin, resolve_rule_paths, rule_matches};
use crate::safety::{self, Scope};
use crate::size::disk_usage;

use super::candidates::{Candidate, Subject, lexical};
use super::matcher::LeftoverIndex;
use super::system::Claimant;
use super::{Confidence, Finding, FindingKind};

/// Names that belong to the desktop or the shell rather than to an app.
const IGNORED_NAMES: &[&str] = &[
    ".bashrc",
    ".bash_profile",
    ".bash_logout",
    ".profile",
    ".zshrc",
    ".zshenv",
    ".zprofile",
    ".xinitrc",
    ".xprofile",
    ".Xresources",
    ".Xauthority",
    ".ICEauthority",
    ".gitconfig",
    ".dir_colors",
    ".face",
    ".face.icon",
    ".directory",
    "gtk-2.0",
    "gtk-3.0",
    "gtk-4.0",
    "gtkrc",
    "gtkrc-2.0",
    "dconf",
    "glib-2.0",
    "fontconfig",
    "pulse",
    "pipewire",
    "kdedefaults",
    "user-dirs.dirs",
    "user-dirs.locale",
    "mimeapps.list",
    "environment.d",
    "systemd",
    "autostart",
    "menus",
    "xsettingsd",
];

fn is_ignored(env: &Env, c: &Candidate) -> bool {
    let Some(name) = c.path.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    if IGNORED_NAMES.contains(&name) {
        return true;
    }
    c.path
        .parent()
        .is_some_and(|p| p == env.unsys(&env.config_home))
        && (name.ends_with("rc") || name.starts_with("plasma"))
}

/// Whether a launcher's program can be found: an absolute path that exists,
/// or a bare name in a trusted directory or `~/.local/bin`.
fn program_resolves(env: &Env, program: &str) -> bool {
    let exists = |p: &Path| std::fs::metadata(env.sys(&p.to_string_lossy())).is_ok();
    if program.starts_with('/') {
        return exists(Path::new(program));
    }
    TRUSTED_DIRS
        .iter()
        .map(|d| Path::new(d).join(program))
        .chain(std::iter::once(
            env.unsys(&env.home).join(".local/bin").join(program),
        ))
        .any(|p| exists(&p))
}

/// What names a rule in messages: its first package, else its first
/// non-empty match key.
fn rule_label(rule: &Rule) -> Option<&str> {
    [
        &rule.packages,
        &rule.flatpaks,
        &rule.appimages,
        &rule.desktops,
    ]
    .into_iter()
    .flatten()
    .map(String::as_str)
    .find(|s| !s.is_empty())
}

/// Leftover candidates that match no installed app. Never `High`: a rule
/// naming an undetected app gives `Medium` (not detected is not uninstalled),
/// and anything modified in the last 30 days is capped at `Low`. Sorted by
/// confidence (highest first), then path. Low findings only with `show_low`.
/// Orphans are never pre-selected.
pub fn find_orphans(idx: &LeftoverIndex, show_low: bool) -> Vec<Finding> {
    find_orphans_at(idx, show_low, SystemTime::now())
}

/// `find_orphans` with an injectable clock.
pub(crate) fn find_orphans_at(
    idx: &LeftoverIndex,
    show_low: bool,
    now: SystemTime,
) -> Vec<Finding> {
    let env = idx.env();
    let apps = &idx.inventory().apps;

    // Resolved paths of every rule, split by whether an installed app uses it.
    let mut live: Vec<PathBuf> = Vec::new();
    let mut gone: Vec<(PathBuf, String)> = Vec::new();
    for rule in idx.rules() {
        let installed = apps.iter().any(|a| rule_matches(rule, a));
        let paths = resolve_rule_paths(env, rule)
            .into_iter()
            .filter_map(|p| lexical(&p));
        if installed {
            live.extend(paths);
        } else if matches!(rule.origin, RuleOrigin::Builtin) {
            let Some(label) = rule_label(rule) else {
                continue;
            };
            let reason = format!("looks like {label} data; {label} was not detected as installed");
            gone.extend(paths.map(|p| (p, reason.clone())));
        }
    }

    // Paths an installed app owns: a candidate holding one of them (a shared
    // parent such as `~/.mozilla`) must stay, or trashing it takes them too.
    let claimed: Vec<PathBuf> = idx
        .candidates()
        .iter()
        .enumerate()
        .filter(|(i, _)| idx.matched_by_any(*i))
        .map(|(_, c)| c.path.clone())
        .collect();

    let mut out: Vec<Finding> = Vec::new();
    for (i, c) in idx.candidates().iter().enumerate() {
        let eligible = matches!(
            c.kind,
            FindingKind::Config
                | FindingKind::Cache
                | FindingKind::Data
                | FindingKind::State
                | FindingKind::DotDir
                | FindingKind::DotFile
                | FindingKind::FlatpakData
                | FindingKind::SnapData
                | FindingKind::Autostart
                | FindingKind::DesktopEntry
        );
        if !eligible
            || idx.matched_by_any(i)
            || is_ignored(env, c)
            || shadowed_by_live(&c.path, &live, &claimed)
        {
            continue;
        }
        let (mut confidence, mut reason) = classify(env, c, &gone);
        if let Some(days) = recent_days(env, &c.path, now) {
            confidence = confidence.min(Confidence::Low);
            reason.push_str(&format!("; modified {days} days ago"));
        }
        if confidence == Confidence::Low && !show_low {
            continue;
        }
        out.push(Finding {
            path: c.path.clone(),
            kind: c.kind,
            confidence,
            shared: false,
            reason,
            size: disk_usage(env, &c.path),
            preselected: false,
            fingerprint: c.fingerprint,
            scope: Scope::Home,
            mime_lines: Vec::new(),
        });
    }
    out.sort_by(|a, b| {
        Reverse(a.confidence)
            .cmp(&Reverse(b.confidence))
            .then_with(|| a.path.cmp(&b.path))
    });
    out
}

/// Unowned real directories directly in `/opt` that no detected app claims
/// (by any name, weak or strong, or a rule path) and that are not the
/// program directory or `location` of one. Never looks in `/etc`. Always
/// `Low`, never pre-selected, root-deleted (`Scope::SystemTree`) and
/// verified now. Empty unless the package database was read completely
/// and `tmpfiles.d` is readable. Sorted by path.
pub fn find_system_orphans(env: &Env, cfg: &Config, idx: &LeftoverIndex) -> Vec<Finding> {
    let inv = idx.inventory();
    let owned = &inv.owned;
    if owned.is_empty() || !owned.is_complete() {
        return Vec::new();
    }
    let Ok(declared) = tmpfiles_declared(env) else {
        return Vec::new();
    };
    let Ok(rd) = std::fs::read_dir(env.sys("/opt")) else {
        return Vec::new();
    };
    let mut names: Vec<String> = rd
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .collect();
    names.sort();
    let mut claimants: Option<Vec<Claimant>> = None;
    let mut out = Vec::new();
    for name in names {
        let path = Path::new("/opt").join(&name);
        let is_real_dir = std::fs::symlink_metadata(env.sys_path(&path)).is_ok_and(|m| m.is_dir());
        if !is_real_dir
            || !safety::is_system_leftover_name(std::ffi::OsStr::new(&name))
            || owned.owns_at_or_below(&path)
            || declared
                .range::<Path, _>((
                    std::ops::Bound::Included(path.as_path()),
                    std::ops::Bound::Unbounded,
                ))
                .next()
                .is_some_and(|first| first.starts_with(&path))
            || inv.apps.iter().any(|a| {
                [&a.program, &a.location]
                    .into_iter()
                    .flatten()
                    .any(|p| p.starts_with(&path))
            })
        {
            continue;
        }
        let claimants = claimants
            .get_or_insert_with(|| inv.apps.iter().map(|a| Claimant::of(idx, a)).collect());
        if claimants.iter().any(|c| c.claim(&path, &name).is_some()) {
            continue;
        }
        let Ok(fingerprint) = safety::verify_now(env, cfg, Scope::SystemTree, &path) else {
            continue;
        };
        out.push(Finding {
            size: disk_usage(env, &path),
            path,
            kind: FindingKind::SystemData,
            confidence: Confidence::Low,
            shared: false,
            reason: "not owned by any package and matched by no app".into(),
            preselected: false,
            scope: Scope::SystemTree,
            fingerprint,
            mime_lines: Vec::new(),
        });
    }
    out
}

/// Whole days since the candidate itself (lstat, no tree walk) was modified,
/// when that is within the last 30 days.
fn recent_days(env: &Env, path: &Path, now: SystemTime) -> Option<u64> {
    let modified = std::fs::symlink_metadata(env.sys_path(path))
        .ok()?
        .modified()
        .ok()?;
    // A modification time in the future counts as just now.
    let age = now.duration_since(modified).unwrap_or_default();
    let days = age.as_secs() / 86_400;
    (days < 30).then_some(days)
}

fn classify(env: &Env, c: &Candidate, gone: &[(PathBuf, String)]) -> (Confidence, String) {
    if let Some((_, reason)) = gone.iter().find(|(p, _)| c.path.starts_with(p)) {
        return (Confidence::Medium, reason.clone());
    }
    match &c.subject {
        Subject::Flatpak(id) => (
            Confidence::Medium,
            format!("data of the Flatpak {id}, which is not installed"),
        ),
        Subject::Launcher(e) => match e.exec_program.as_deref() {
            Some(program) if !program_resolves(env, program) => (
                Confidence::Medium,
                format!("launcher runs {program}, which was not found"),
            ),
            _ => (Confidence::Low, "launcher of no installed app".to_string()),
        },
        _ => (
            Confidence::Low,
            "does not match any installed app".to_string(),
        ),
    }
}

/// Whether trashing `candidate` would touch something an installed app owns:
/// it is equal to or inside a `live` rule path, or it holds (is an ancestor
/// of) a `live` rule path or a path in `claimed`.
fn shadowed_by_live(candidate: &Path, live: &[PathBuf], claimed: &[PathBuf]) -> bool {
    live.iter()
        .any(|p| candidate.starts_with(p) || p.starts_with(candidate))
        || claimed.iter().any(|p| p.starts_with(candidate))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shadowed_by_live_table() {
        let p = |s: &str| PathBuf::from(s);
        let live = [p("/h/.mozilla/firefox")];
        assert!(shadowed_by_live(Path::new("/h/.mozilla"), &live, &[]));
        assert!(shadowed_by_live(
            Path::new("/h/.mozilla/firefox/x"),
            &live,
            &[]
        ));
        assert!(!shadowed_by_live(Path::new("/h/.mozillax"), &live, &[]));
        assert!(shadowed_by_live(
            Path::new("/h/.mozilla"),
            &[],
            &[p("/h/.mozilla/firefox")]
        ));
        assert!(!shadowed_by_live(Path::new("/h/.mozilla"), &[], &[]));
    }
}
