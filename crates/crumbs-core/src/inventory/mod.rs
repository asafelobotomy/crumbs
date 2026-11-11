//! Whole-system app discovery across all sources.

use std::collections::HashSet;

use serde::Serialize;

use crate::command::CommandRunner;
use crate::config::Config;
use crate::desktop::discover_desktop_entries;
use crate::env::Env;
use crate::error::SourceError;
use crate::model::{App, SourceKind};
use crate::ownership::OwnedIndex;
use crate::sources::appimage::{AppImageSource, normalize_name};
use crate::sources::flatpak::FlatpakSource;
use crate::sources::pacman::PacmanSource;
use crate::sources::unmanaged::list_unmanaged;
use crate::sources::{AppSource, ListOptions, SourceContext};

#[derive(Serialize, Debug, Clone, PartialEq)]
pub struct SourceStatus {
    pub source: SourceKind,
    pub available: bool,
    pub app_count: usize,
    pub skipped_reason: Option<String>,
}

#[derive(Serialize, Debug, Clone, PartialEq)]
pub struct Inventory {
    pub apps: Vec<App>,
    pub statuses: Vec<SourceStatus>,
    pub errors: Vec<SourceError>,
    /// Package-owned files and directories; not part of the JSON output.
    #[serde(skip)]
    pub owned: OwnedIndex,
}

pub fn default_sources() -> Vec<Box<dyn AppSource>> {
    vec![
        Box::new(PacmanSource),
        Box::new(crate::sources::dpkg::DpkgSource),
        Box::new(crate::sources::rpm::RpmSource),
        Box::new(FlatpakSource),
        Box::new(crate::sources::snap::SnapSource),
        Box::new(AppImageSource),
    ]
}

fn unavailable_reason(kind: SourceKind) -> &'static str {
    match kind {
        SourceKind::Pacman => "no pacman database",
        SourceKind::Dpkg => "no dpkg database",
        SourceKind::Rpm => "rpm not installed",
        SourceKind::Flatpak => "not installed",
        SourceKind::Snap => "snap not installed",
        _ => "not available",
    }
}

/// Lists apps from every available source, then unmanaged apps when a
/// package database was read successfully.
pub fn discover(
    env: &Env,
    runner: &dyn CommandRunner,
    opts: &ListOptions,
    cfg: &Config,
) -> Inventory {
    let desktop = discover_desktop_entries(env);
    let cx = SourceContext {
        env,
        runner,
        desktop: &desktop,
        opts,
        config: cfg,
    };

    let mut apps = Vec::new();
    let mut statuses = Vec::new();
    let mut errors = Vec::new();
    let mut owned = HashSet::new();
    let mut owned_dirs = HashSet::new();
    let mut claimed = HashSet::new();
    let mut package_db_listed = false;
    let mut package_db_incomplete = false;

    for source in default_sources() {
        let kind = source.kind();
        if !source.available(env) {
            statuses.push(SourceStatus {
                source: kind,
                available: false,
                app_count: 0,
                skipped_reason: Some(unavailable_reason(kind).into()),
            });
            continue;
        }
        match source.list(&cx) {
            Ok(listing) => {
                if source.is_package_db() {
                    if listing.errors.is_empty() {
                        package_db_listed = true;
                    } else {
                        package_db_incomplete = true;
                    }
                }
                statuses.push(SourceStatus {
                    source: kind,
                    available: true,
                    app_count: listing.apps.len(),
                    skipped_reason: None,
                });
                owned.extend(listing.owned_paths);
                owned_dirs.extend(listing.owned_dirs);
                claimed.extend(listing.claimed_desktop_ids);
                errors.extend(listing.errors);
                apps.extend(listing.apps);
            }
            Err(e) => {
                statuses.push(SourceStatus {
                    source: kind,
                    available: true,
                    app_count: 0,
                    skipped_reason: None,
                });
                if source.is_package_db() {
                    package_db_incomplete = true;
                }
                errors.push(SourceError {
                    source: kind,
                    message: e.to_string(),
                });
            }
        }
    }
    // An incomplete ownership set must never be used to classify unmanaged apps.
    package_db_listed &= !package_db_incomplete;

    if package_db_listed {
        let listing = list_unmanaged(&cx, &owned, &claimed);
        statuses.push(SourceStatus {
            source: SourceKind::Unmanaged,
            available: true,
            app_count: listing.apps.len(),
            skipped_reason: None,
        });
        apps.extend(listing.apps);
    } else {
        statuses.push(SourceStatus {
            source: SourceKind::Unmanaged,
            available: false,
            app_count: 0,
            skipped_reason: Some(
                if package_db_incomplete {
                    "package database incomplete"
                } else {
                    "no supported package database found"
                }
                .into(),
            ),
        });
    }

    apps.sort_by(|a, b| {
        a.display_name
            .to_lowercase()
            .cmp(&b.display_name.to_lowercase())
            .then_with(|| a.id.cmp(&b.id))
    });
    Inventory {
        apps,
        statuses,
        errors,
        owned: OwnedIndex::new(owned, owned_dirs, package_db_listed),
    }
}

#[derive(Debug)]
pub enum Resolved<'a> {
    Found(&'a App),
    Ambiguous(Vec<&'a App>),
    NotFound,
}

/// Finds an app by id, name, desktop ID, rDNS ID, display name or loose name.
/// The first matching rule decides; several matches in it are ambiguous.
pub fn find_app<'a>(inv: &'a Inventory, query: &str) -> Resolved<'a> {
    resolve(inv, query, true)
}

/// Like [`find_app`] without the last (normalized-name) rule: only exact
/// matches. Destructive commands use this.
pub fn find_app_exact<'a>(inv: &'a Inventory, query: &str) -> Resolved<'a> {
    resolve(inv, query, false)
}

/// Whether `id` is a desktop file id a launcher may hand over, such as uQ's
/// "Uninstall…" app action: `^[A-Za-z0-9._-]{1,200}\.desktop$`, not starting
/// with `-` or `.` (so it never reads as an option or a hidden file).
pub fn valid_desktop_id(id: &str) -> bool {
    let Some(stem) = id.strip_suffix(".desktop") else {
        return false;
    };
    (1..=200).contains(&stem.len())
        && !stem.starts_with(['-', '.'])
        && stem
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

/// Finds the installed app whose launchers include the desktop file id `id`
/// (`firefox.desktop`, or `kde-foo.desktop` for a subdirectory), and nothing
/// else: no app id, name or display name is tried, so a launcher can only
/// ever name the app its own entry belongs to. An id that is not
/// [`valid_desktop_id`] finds nothing.
pub fn find_app_by_desktop_id<'a>(inv: &'a Inventory, id: &str) -> Resolved<'a> {
    if !valid_desktop_id(id) {
        return Resolved::NotFound;
    }
    let mut hits: Vec<&App> = inv
        .apps
        .iter()
        .filter(|a| a.desktop_ids.iter().any(|d| d == id))
        .collect();
    match hits.len() {
        0 => Resolved::NotFound,
        1 => Resolved::Found(hits.remove(0)),
        _ => Resolved::Ambiguous(hits),
    }
}

fn resolve<'a>(inv: &'a Inventory, query: &str, loose: bool) -> Resolved<'a> {
    let with_ext = if query.ends_with(".desktop") {
        query.to_string()
    } else {
        format!("{query}.desktop")
    };
    let lower = query.to_lowercase();
    let norm = normalize_name(query);

    let rules: [&dyn Fn(&App) -> bool; 6] = [
        &|a| a.id == query,
        &|a| a.name == query,
        &|a| a.desktop_ids.iter().any(|d| d == query || *d == with_ext),
        &|a| a.rdns_id.as_deref() == Some(query),
        &|a| a.display_name.to_lowercase() == lower,
        &|a| {
            !norm.is_empty()
                && (normalize_name(&a.name) == norm || normalize_name(&a.display_name) == norm)
        },
    ];
    let count = if loose { rules.len() } else { rules.len() - 1 };
    for rule in &rules[..count] {
        let mut hits: Vec<&App> = inv.apps.iter().filter(|a| rule(a)).collect();
        match hits.len() {
            0 => continue,
            1 => return Resolved::Found(hits.remove(0)),
            _ => return Resolved::Ambiguous(hits),
        }
    }
    Resolved::NotFound
}

#[cfg(test)]
mod tests;
