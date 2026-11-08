//! Orphans: leftovers that no installed app claims.

use std::cmp::Reverse;
use std::path::{Path, PathBuf};

use crate::command::TRUSTED_DIRS;
use crate::env::Env;
use crate::rules::{Rule, RuleOrigin, resolve_rule_paths, rule_matches};
use crate::size::disk_usage;

use super::candidates::{Candidate, Subject, lexical};
use super::matcher::LeftoverIndex;
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

/// Leftover candidates that match no installed app. `High` only when a
/// builtin rule names them and its app is not installed. Sorted by
/// confidence (highest first), then path. Low findings only with `show_low`.
/// Orphans are never pre-selected.
pub fn find_orphans(idx: &LeftoverIndex, show_low: bool) -> Vec<Finding> {
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
            let reason = format!("belongs to {label}, which is not installed");
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
        let (confidence, reason) = classify(env, c, &gone);
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

fn classify(env: &Env, c: &Candidate, gone: &[(PathBuf, String)]) -> (Confidence, String) {
    if let Some((_, reason)) = gone.iter().find(|(p, _)| c.path.starts_with(p)) {
        return (Confidence::High, reason.clone());
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
