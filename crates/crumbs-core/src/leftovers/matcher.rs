//! The leftover index (built once per inventory) and per-app lookup.

use std::cmp::Reverse;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::config::Config;
use crate::desktop::DesktopEntry;
use crate::env::Env;
use crate::inventory::Inventory;
use crate::model::{App, SourceKind};
use crate::rules::{Rule, RuleOrigin, resolve_rule_paths, rules_for};
use crate::safety::protected_paths;
use crate::size::{DiskUsage, disk_usage};

use super::candidates::{Bases, Candidate, Subject, admit, enumerate, holds_container, lexical};
use super::score::{Idents, score_name};
use super::{Confidence, Finding, FindingKind};

/// One app's claim on a candidate.
#[derive(Debug, Clone)]
struct Match {
    app: usize,
    confidence: Confidence,
    reason: String,
}

/// Every candidate leftover on the system, with which inventory apps match
/// each one and how strongly. Holds copies of what it was built from.
#[derive(Debug, Clone)]
pub struct LeftoverIndex {
    env: Env,
    rules: Vec<Rule>,
    inv: Inventory,
    candidates: Vec<Candidate>,
    /// Parallel to `candidates`; at most one entry per app.
    matches: Vec<Vec<Match>>,
}

impl LeftoverIndex {
    /// Builds the index. `inv` must be discovered with
    /// `ListOptions { include_all: true }`: sharing is only detected against
    /// apps in the inventory, so a filtered inventory under-reports `shared`
    /// and pre-selects paths that other packages still use.
    pub fn build(env: &Env, cfg: &Config, rules: &[Rule], inv: &Inventory) -> LeftoverIndex {
        let mut candidates = enumerate(env, cfg);
        let idents: Vec<Idents> = inv.apps.iter().map(Idents::of).collect();
        let appimage_launchers: HashSet<&str> = inv
            .apps
            .iter()
            .filter(|a| a.source == SourceKind::AppImage)
            .flat_map(|a| a.desktop_ids.iter().map(String::as_str))
            .collect();

        let mut matches: Vec<Vec<Match>> = candidates
            .iter()
            .map(|c| {
                inv.apps
                    .iter()
                    .zip(&idents)
                    .enumerate()
                    .filter_map(|(i, (app, ids))| {
                        let (confidence, reason) = score(c, app, ids, &appimage_launchers)?;
                        Some(Match {
                            app: i,
                            confidence: cap(c, app, confidence),
                            reason,
                        })
                    })
                    .collect()
            })
            .collect();

        let bases = Bases::of(env);
        let protected = protected_paths(env, cfg);
        let containers = bases.containers();
        let mut by_path: HashMap<PathBuf, usize> = candidates
            .iter()
            .enumerate()
            .map(|(i, c)| (c.path.clone(), i))
            .collect();
        for (ai, app) in inv.apps.iter().enumerate() {
            for rule in rules_for(rules, app) {
                let reason = match &rule.origin {
                    RuleOrigin::Builtin => "listed in a curated rule".to_string(),
                    RuleOrigin::User(f) => format!("listed in rule file {}", f.display()),
                };
                for raw in resolve_rule_paths(env, rule) {
                    let Some(path) = lexical(&raw) else { continue };
                    if holds_container(&containers, &path) {
                        continue;
                    }
                    let ci = match by_path.get(&path) {
                        Some(&ci) => ci,
                        None => {
                            let kind = bases.kind_for(&path);
                            let subject = Subject::RuleOnly;
                            let Some(c) = admit(env, cfg, &protected, path.clone(), kind, subject)
                            else {
                                continue;
                            };
                            candidates.push(c);
                            matches.push(Vec::new());
                            by_path.insert(path, candidates.len() - 1);
                            candidates.len() - 1
                        }
                    };
                    let m = Match {
                        app: ai,
                        confidence: cap(&candidates[ci], app, Confidence::High),
                        reason: reason.clone(),
                    };
                    match matches[ci].iter_mut().find(|m| m.app == ai) {
                        Some(old) if old.confidence > m.confidence => {}
                        Some(old) => *old = m,
                        None => matches[ci].push(m),
                    }
                }
            }
        }

        LeftoverIndex {
            env: env.clone(),
            rules: rules.to_vec(),
            inv: inv.clone(),
            candidates,
            matches,
        }
    }

    // Read by orphan detection.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn candidates(&self) -> &[Candidate] {
        &self.candidates
    }

    /// Whether any inventory app matches candidate `i`, at any confidence.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn matched_by_any(&self, i: usize) -> bool {
        self.matches.get(i).is_some_and(|m| !m.is_empty())
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn inventory(&self) -> &Inventory {
        &self.inv
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn rules(&self) -> &[Rule] {
        &self.rules
    }
}

/// A Flatpak keeps its data in `~/.var/app/<id>`; anything else it matches
/// (other than by its own desktop ID or full app ID) is at most Medium.
fn cap(c: &Candidate, app: &App, confidence: Confidence) -> Confidence {
    let by_id = match &c.subject {
        Subject::Flatpak(_) | Subject::Icon(_) | Subject::Mime(_) => true,
        Subject::Launcher(e) => app.desktop_ids.contains(&e.id),
        _ => false,
    };
    if app.source == SourceKind::Flatpak && !by_id {
        confidence.min(Confidence::Medium)
    } else {
        confidence
    }
}

/// How strongly `app` claims candidate `c`, ignoring rules and sharing.
fn score(
    c: &Candidate,
    app: &App,
    ids: &Idents,
    appimage_launchers: &HashSet<&str>,
) -> Option<(Confidence, String)> {
    match &c.subject {
        Subject::Name(name) => score_name(name, ids),
        Subject::Flatpak(id) => (app.source == SourceKind::Flatpak
            && app.rdns_id.as_deref() == Some(id))
        .then(|| (Confidence::High, format!("data of the Flatpak {id}"))),
        Subject::Launcher(e) => score_launcher(e, app, ids, appimage_launchers),
        Subject::Icon(stem) => ids
            .icon_names()
            .any(|n| n == stem)
            .then(|| (Confidence::High, format!("icon named after {stem}"))),
        Subject::Service(Some(program)) => {
            let base = Path::new(program).file_name()?.to_str()?;
            (ids.has_exec(base) || app.location.as_deref() == Some(Path::new(program)))
                .then(|| (Confidence::Medium, format!("user service runs {program}")))
        }
        Subject::Service(None) | Subject::Snap(_) | Subject::RuleOnly => None,
        Subject::Mime(lines) => lines
            .iter()
            .any(|l| l.desktop_ids.iter().any(|d| app.desktop_ids.contains(d)))
            .then(|| (Confidence::High, "file-type associations".to_string())),
    }
}

/// A launcher listed in an AppImage's `desktop_ids` belongs to that AppImage
/// alone; other apps never match it.
fn score_launcher(
    e: &DesktopEntry,
    app: &App,
    ids: &Idents,
    appimage_launchers: &HashSet<&str>,
) -> Option<(Confidence, String)> {
    if app.desktop_ids.contains(&e.id) {
        return Some((Confidence::High, format!("launcher for {}", e.id)));
    }
    if appimage_launchers.contains(e.id.as_str()) {
        return None;
    }
    let program = e.exec_program.as_deref()?;
    if app.location.as_deref() == Some(Path::new(program)) {
        return Some((Confidence::High, format!("launcher runs {program}")));
    }
    let base = Path::new(program).file_name()?.to_str()?;
    (!e.is_flatpak_entry && ids.has_exec(base))
        .then(|| (Confidence::Medium, format!("launcher runs {base}")))
}

/// Leftovers of `app`, which must be an app of the index's inventory (found
/// by `id`; any other app has none). Sorted by confidence (highest first),
/// then path. Low findings only with `show_low`.
pub fn find_leftovers(idx: &LeftoverIndex, app: &App, show_low: bool) -> Vec<Finding> {
    let apps = &idx.inv.apps;
    let Some(ai) = apps.iter().position(|a| a.id == app.id) else {
        return Vec::new();
    };
    let mut found: Vec<(Finding, &Candidate)> = Vec::new();
    for (c, ms) in idx.candidates.iter().zip(&idx.matches) {
        let Some(m) = ms.iter().find(|m| m.app == ai) else {
            continue;
        };
        let mut mime_lines = Vec::new();
        let others: Vec<usize> = if let Subject::Mime(lines) = &c.subject {
            // The file is edited line by line; it is shared when a line we
            // would remove also names another installed app.
            let mine: Vec<_> = lines
                .iter()
                .filter(|l| l.desktop_ids.iter().any(|d| app.desktop_ids.contains(d)))
                .collect();
            mime_lines = mine.iter().map(|l| l.line.clone()).collect();
            (0..apps.len())
                .filter(|&o| {
                    o != ai
                        && mine.iter().any(|l| {
                            l.desktop_ids
                                .iter()
                                .any(|d| apps[o].desktop_ids.contains(d))
                        })
                })
                .collect()
        } else if m.confidence >= Confidence::Medium {
            // Another app's Medium+ claim on this path, or on anything inside
            // it, means trashing it would take that app's data too.
            let inside = idx
                .candidates
                .iter()
                .zip(&idx.matches)
                .filter(|(d, _)| d.path != c.path && d.path.starts_with(&c.path))
                .flat_map(|(_, dm)| dm);
            let mut v: Vec<usize> = ms
                .iter()
                .chain(inside)
                .filter(|o| o.app != ai && o.confidence >= Confidence::Medium)
                .map(|o| o.app)
                .collect();
            v.sort_unstable();
            v.dedup();
            v
        } else {
            Vec::new()
        };
        let shared = !others.is_empty();
        let mut confidence = m.confidence;
        let mut reason = m.reason.clone();
        if shared {
            confidence = confidence.downgraded();
            let mut names: Vec<&str> = others
                .iter()
                .map(|&o| apps[o].display_name.as_str())
                .collect();
            names.sort_unstable();
            names.dedup();
            reason.push_str(&format!("; shared with {}", names.join(", ")));
        }
        if confidence == Confidence::Low && !show_low {
            continue;
        }
        let finding = Finding {
            path: c.path.clone(),
            kind: c.kind,
            confidence,
            shared,
            reason,
            size: DiskUsage::default(),
            preselected: confidence == Confidence::High && !shared,
            fingerprint: c.fingerprint,
            mime_lines,
        };
        found.push((finding, c));
    }

    // Something inside an equally or more confident finding goes with it,
    // unless it is shared (it must stay visible with its own flag).
    let covered: Vec<bool> = found
        .iter()
        .map(|(f, _)| {
            !f.shared
                && found.iter().any(|(g, _)| {
                    g.path != f.path && f.path.starts_with(&g.path) && g.confidence >= f.confidence
                })
        })
        .collect();
    let mut out: Vec<Finding> = found
        .into_iter()
        .zip(covered)
        .filter(|(_, covered)| !covered)
        .map(|((mut f, c), _)| {
            if c.kind != FindingKind::MimeAssoc {
                f.size = disk_usage(&idx.env, &c.path);
            }
            f
        })
        .collect();
    out.sort_by(|a, b| {
        Reverse(a.confidence)
            .cmp(&Reverse(b.confidence))
            .then_with(|| a.path.cmp(&b.path))
    });
    out
}
