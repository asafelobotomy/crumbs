//! Leftovers in `/etc` and `/opt`: direct children no package owns, named
//! after an app. They are never High and never pre-selected; root deletes
//! them only when the user picks them.

use std::collections::HashSet;
use std::fs;
use std::path::Path;

use crate::config::Config;
use crate::env::Env;
use crate::model::{App, SourceKind};
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

/// How `app` claims the entry `name` at `path`, if it does: Medium for an
/// exact name or a rule path, Low for a normalised match.
fn claim(idx: &LeftoverIndex, app: &App, path: &Path, name: &str) -> Option<(Confidence, String)> {
    for rule in rules_for(idx.rules(), app) {
        if rule
            .paths
            .iter()
            .filter_map(|p| expand_system_rule_path(p))
            .any(|p| p == path)
        {
            let reason = match &rule.origin {
                RuleOrigin::Builtin => "listed in a curated rule".to_string(),
                RuleOrigin::User(f) => format!("listed in rule file {}", f.display()),
            };
            return Some((Confidence::Medium, reason));
        }
    }
    let (names, keys) = app_name_sets(app);
    if names.iter().any(|n| n == name) {
        return Some((
            Confidence::Medium,
            format!("named after the app ('{name}')"),
        ));
    }
    let entry = NameKey::new(name);
    keys.iter()
        .find(|k| k.norm == entry.norm || k.norm == entry.unversioned)
        .map(|k| {
            (
                Confidence::Low,
                format!("name resembles the app name '{}'", k.raw),
            )
        })
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
            {
                continue;
            }
            let Some((mut confidence, mut reason)) = claim(idx, &inv.apps[ai], &path, &name) else {
                continue;
            };
            // Another installed app with an equally firm claim shares it.
            let others: HashSet<&str> = inv
                .apps
                .iter()
                .enumerate()
                .filter(|&(o, _)| o != ai)
                .filter(|(_, a)| {
                    claim(idx, a, &path, &name).is_some_and(|(c, _)| c >= Confidence::Medium)
                })
                .map(|(_, a)| a.display_name.as_str())
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
