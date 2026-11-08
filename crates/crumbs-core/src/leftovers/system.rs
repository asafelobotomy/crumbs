//! Leftovers in `/etc` and `/opt`: direct children no package owns, named
//! after an app. They are never High and never pre-selected; root deletes
//! them only when the user picks them.

use std::cell::OnceCell;
use std::collections::HashSet;
use std::fs;
use std::ops::Bound;
use std::path::{Path, PathBuf};

use crate::config::Config;
use crate::env::Env;
use crate::model::{App, SourceKind};
use crate::ownership::tmpfiles_declared;
use crate::rules::{RuleOrigin, expand_system_rule_path, rules_for};
use crate::safety::{self, Scope};
use crate::size::disk_usage;
use crate::sources::appimage::split_appimage_name;

use super::candidates::NameKey;
use super::matcher::LeftoverIndex;
use super::score::{desktop_stem, usable};
use super::{Confidence, Finding, FindingKind};

/// An app's exact names (`name`, desktop-id stems, reverse-DNS id, `Exec`
/// basenames) and, parallel to them, their normalised keys. Names too
/// generic to say anything (the home stoplist, under three characters) are
/// dropped. Independent of what is on disk.
pub(crate) fn app_name_sets(app: &App) -> (Vec<String>, Vec<NameKey>) {
    let name = if app.source == SourceKind::AppImage {
        split_appimage_name(&app.name).0
    } else {
        app.name.clone()
    };
    let mut names: Vec<String> = Vec::new();
    let mut add = |raw: &str| {
        let key = NameKey::new(raw);
        if usable(raw, &key.norm) && !names.iter().any(|n| n == raw) {
            names.push(raw.to_string());
        }
    };
    add(&name);
    for d in &app.desktop_ids {
        add(desktop_stem(d));
    }
    if let Some(rdns) = app.rdns_id.as_deref() {
        add(rdns);
    }
    for e in &app.exec_names {
        add(e);
    }
    let keys = names.iter().map(|n| NameKey::new(n)).collect();
    (names, keys)
}

/// Like [`app_name_sets`] but only names the user chose: no `Exec`
/// basenames (derived from the program path, so they prove nothing about a
/// path) and with the desktop entry's display `Name=` added.
pub(crate) fn strong_name_sets(app: &App) -> (Vec<String>, Vec<NameKey>) {
    let mut no_exec = app.clone();
    no_exec.exec_names.clear();
    let (mut names, _) = app_name_sets(&no_exec);
    let key = NameKey::new(&app.display_name);
    if usable(&app.display_name, &key.norm) && !names.contains(&app.display_name) {
        names.push(app.display_name.clone());
    }
    let keys = names.iter().map(|n| NameKey::new(n)).collect();
    (names, keys)
}

/// What one app claims in `/etc` and `/opt`, computed once per app.
pub(crate) struct Claimant {
    /// `(path, reason)` of rule paths.
    rule_paths: Vec<(PathBuf, String)>,
    /// Names from the app name, desktop-id stems and reverse-DNS id.
    strong: Vec<String>,
    /// Every exact name, `Exec` basenames included.
    all: Vec<String>,
    keys: Vec<NameKey>,
}

impl Claimant {
    pub(crate) fn of(idx: &LeftoverIndex, app: &App) -> Claimant {
        let mut rule_paths = Vec::new();
        for rule in rules_for(idx.rules(), app) {
            let reason = match &rule.origin {
                RuleOrigin::Builtin => "listed in a curated rule".to_string(),
                RuleOrigin::User(f) => format!("listed in rule file {}", f.display()),
            };
            rule_paths.extend(
                rule.paths
                    .iter()
                    .filter_map(|p| expand_system_rule_path(p))
                    .map(|p| (p, reason.clone())),
            );
        }
        let (all, keys) = app_name_sets(app);
        let mut no_exec = app.clone();
        no_exec.exec_names.clear();
        let (strong, _) = app_name_sets(&no_exec);
        Claimant {
            rule_paths,
            strong,
            all,
            keys,
        }
    }

    /// How the app claims the entry `name` at `path`, if it does: Medium
    /// for an exact name or a rule path, Low for a normalised match or a
    /// match on an `Exec` basename alone.
    pub(crate) fn claim(&self, path: &Path, name: &str) -> Option<(Confidence, String)> {
        if let Some((_, reason)) = self.rule_paths.iter().find(|(p, _)| p == path) {
            return Some((Confidence::Medium, reason.clone()));
        }
        if self.strong.iter().any(|n| n == name) {
            return Some((
                Confidence::Medium,
                format!("named after the app ('{name}')"),
            ));
        }
        if self.all.iter().any(|n| n == name) {
            return Some((Confidence::Low, format!("named after the program '{name}'")));
        }
        let entry = NameKey::new(name);
        self.keys
            .iter()
            .find(|k| k.norm == entry.norm || k.norm == entry.unversioned)
            .map(|k| {
                (
                    Confidence::Low,
                    format!("name resembles the app name '{}'", k.raw),
                )
            })
    }
}

/// Unowned direct children of `/etc` and `/opt` that `app` claims, for the
/// user to pick. Empty unless the package database was read completely:
/// without it "unowned" is a guess. Every finding is root-deleted
/// (`Scope::SystemTree`), not pre-selected, at most Medium, and has passed
/// [`safety::verify_now`] now. Sorted by confidence (highest first), then
/// path. Low findings only with `show_low`.
pub fn find_system_leftovers(
    env: &Env,
    cfg: &Config,
    idx: &LeftoverIndex,
    app: &App,
    show_low: bool,
) -> Vec<Finding> {
    let inv = idx.inventory();
    let owned = &inv.owned;
    if owned.is_empty() || !owned.is_complete() {
        return Vec::new();
    }
    let Some(ai) = inv.apps.iter().position(|a| a.id == app.id) else {
        return Vec::new();
    };
    // A package's runtime directories (tmpfiles.d) are never leftovers.
    let Ok(declared) = tmpfiles_declared(env) else {
        return Vec::new();
    };
    let mine = Claimant::of(idx, &inv.apps[ai]);
    // Other apps' claim data is built only once this app claims an entry.
    let others: OnceCell<Vec<(&App, Claimant)>> = OnceCell::new();
    let mut out: Vec<Finding> = Vec::new();
    for (dir, kind) in [
        ("/etc", FindingKind::SystemConfig),
        ("/opt", FindingKind::SystemData),
    ] {
        let Ok(rd) = fs::read_dir(env.sys(dir)) else {
            continue;
        };
        let mut names: Vec<String> = rd
            .flatten()
            .filter_map(|e| e.file_name().into_string().ok())
            .collect();
        names.sort();
        for name in names {
            let path = Path::new(dir).join(&name);
            if !safety::is_system_leftover_name(std::ffi::OsStr::new(&name))
                || owned.owns_at_or_below(&path)
                || declared
                    .range::<Path, _>((Bound::Included(path.as_path()), Bound::Unbounded))
                    .next()
                    .is_some_and(|first| first.starts_with(&path))
            {
                continue;
            }
            let Some((mut confidence, mut reason)) = mine.claim(&path, &name) else {
                continue;
            };
            // Any claim by another installed app shares it.
            let others = others.get_or_init(|| {
                inv.apps
                    .iter()
                    .enumerate()
                    .filter(|&(o, _)| o != ai)
                    .map(|(_, a)| (a, Claimant::of(idx, a)))
                    .collect()
            });
            let others: HashSet<&str> = others
                .iter()
                .filter(|(_, c)| c.claim(&path, &name).is_some())
                .map(|(a, _)| a.display_name.as_str())
                .collect();
            let shared = !others.is_empty();
            if shared {
                confidence = confidence.downgraded();
                let mut v: Vec<&str> = others.into_iter().collect();
                v.sort_unstable();
                reason.push_str(&format!("; shared with {}", v.join(", ")));
            }
            if confidence == Confidence::Low && !show_low {
                continue;
            }
            let Ok(fingerprint) = safety::verify_now(env, cfg, Scope::SystemTree, &path) else {
                continue;
            };
            out.push(Finding {
                size: disk_usage(env, &path),
                path,
                kind,
                confidence,
                shared,
                reason,
                preselected: false,
                scope: Scope::SystemTree,
                fingerprint,
                mime_lines: Vec::new(),
            });
        }
    }
    out.sort_by(|a, b| {
        b.confidence
            .cmp(&a.confidence)
            .then_with(|| a.path.cmp(&b.path))
    });
    out
}
