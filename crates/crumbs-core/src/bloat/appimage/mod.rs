//! `appimage-old`: superseded AppImage versions.
//!
//! AppImages are grouped by normalized base name. Within a group the highest
//! version is kept, as is any version a launcher (menu or autostart entry)
//! runs and any version that is running now. A version that is missing or
//! doesn't parse is never old.

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use super::cache::new_item;
use super::{BloatItem, Category, ScanContext, Scanner};
use crate::desktop::{discover_desktop_entries, parse_desktop};
use crate::error::CoreError;
use crate::executor::running_processes;
use crate::model::{App, SourceKind};
use crate::sources::appimage::{normalize_name, split_appimage_name};

/// Compares two versions component by component, splitting on `.`, `-` and
/// `_`. Components that are both numbers compare numerically, others
/// lexically; a number against a tag is incomparable (`None`). A missing component is less than a numeric one, but greater
/// than a tag such as `beta`, so `2.0` is newer than `2.0-beta`. `None` when
/// either version is empty or the two are incomparable.
pub fn compare_versions(a: &str, b: &str) -> Option<Ordering> {
    if a.is_empty() || b.is_empty() {
        return None;
    }
    let split = |s: &str| -> Vec<String> {
        s.split(['.', '-', '_'])
            .filter(|c| !c.is_empty())
            .map(String::from)
            .collect()
    };
    let (a, b) = (split(a), split(b));
    let is_num = |c: &str| c.bytes().all(|b| b.is_ascii_digit());
    for i in 0..a.len().max(b.len()) {
        let ord = match (a.get(i), b.get(i)) {
            (Some(x), Some(y)) if is_num(x) && is_num(y) => {
                let (x, y) = (x.trim_start_matches('0'), y.trim_start_matches('0'));
                x.len().cmp(&y.len()).then_with(|| x.cmp(y))
            }
            // A number against a tag can't be ordered.
            (Some(x), Some(y)) if is_num(x) != is_num(y) => return None,
            (Some(x), Some(y)) => x.cmp(y),
            (Some(x), None) => {
                if is_num(x) {
                    Ordering::Greater
                } else {
                    Ordering::Less
                }
            }
            (None, Some(y)) => {
                if is_num(y) {
                    Ordering::Less
                } else {
                    Ordering::Greater
                }
            }
            (None, None) => Ordering::Equal,
        };
        if ord != Ordering::Equal {
            return Some(ord);
        }
    }
    Some(Ordering::Equal)
}

/// An AppImage in a group: app, file name, and its version if comparable.
type Member<'a> = (&'a App, String, Option<&'a str>);

/// In-system programs that menu or autostart entries execute, plus their
/// canonical (symlink-resolved) in-system forms.
fn launcher_targets(cx: &ScanContext) -> HashSet<PathBuf> {
    let home = cx.env.unsys(&cx.env.home);
    let mut programs: Vec<String> = discover_desktop_entries(cx.env)
        .into_iter()
        .filter_map(|e| e.exec_program)
        .collect();
    if let Ok(rd) = std::fs::read_dir(cx.env.config_home.join("autostart")) {
        for entry in rd.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if !name.ends_with(".desktop") {
                continue;
            }
            if let Ok(bytes) = std::fs::read(entry.path())
                && let Some(p) = parse_desktop(&name, &entry.path(), &bytes).exec_program
            {
                programs.push(p);
            }
        }
    }
    let mut targets = HashSet::new();
    for p in programs {
        let target = match p.strip_prefix("~/") {
            Some(rest) => home.join(rest),
            None => PathBuf::from(p),
        };
        if target.is_absolute()
            && let Ok(real) = cx.env.sys_path(&target).canonicalize()
        {
            targets.insert(cx.env.unsys(&real));
        }
        targets.insert(target);
    }
    targets
}

pub struct AppimageOldScanner;

impl Scanner for AppimageOldScanner {
    fn category(&self) -> Category {
        Category::AppimageOld
    }

    fn scan(&self, cx: &ScanContext) -> Result<Vec<BloatItem>, CoreError> {
        let mut groups: HashMap<String, Vec<Member>> = HashMap::new();
        for app in cx
            .inv
            .apps
            .iter()
            .filter(|a| a.source == SourceKind::AppImage)
        {
            let Some(file) = app
                .location
                .as_deref()
                .and_then(|l| l.file_name())
                .map(|n| n.to_string_lossy().into_owned())
            else {
                continue;
            };
            let (base, _) = split_appimage_name(&file);
            let version = app
                .version
                .as_deref()
                .filter(|v| compare_versions(v, v).is_some());
            groups
                .entry(normalize_name(&base))
                .or_default()
                .push((app, file, version));
        }

        let targets = launcher_targets(cx);
        let mut out = Vec::new();
        for members in groups.values() {
            for (app, file, version) in members {
                let Some(version) = version else { continue };
                // Old only when some comparable sibling is strictly newer.
                let superseded = members.iter().any(|(_, _, other)| {
                    other.and_then(|o| compare_versions(version, o)) == Some(Ordering::Less)
                });
                if !superseded {
                    continue;
                }
                let Some(location) = app.location.clone() else {
                    continue;
                };
                let canonical = cx
                    .env
                    .sys_path(&location)
                    .canonicalize()
                    .map(|p| cx.env.unsys(&p));
                if targets.contains(&location) || canonical.is_ok_and(|c| targets.contains(&c)) {
                    continue;
                }
                if !running_processes(cx.env, app).is_empty() {
                    continue;
                }
                if let Some(mut item) =
                    new_item(cx, Category::AppimageOld, location, file.clone(), false)
                {
                    item.app_id = Some(app.id.clone());
                    out.push(item);
                }
            }
        }
        out.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(out)
    }
}

#[cfg(test)]
mod tests;
