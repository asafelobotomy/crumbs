//! `cache` and `thumbnails`: the user's cache directory, one item per app.

use std::ffi::OsString;
use std::fs;
use std::path::PathBuf;

use super::dev::dev_cache_paths;
use super::{BloatItem, Category, ItemAction, ScanContext, Scanner};
use crate::error::CoreError;
use crate::executor::running_processes;
use crate::leftovers::Confidence;
use crate::leftovers::candidates::NameKey;
use crate::leftovers::score::{Idents, score_name};
use crate::model::App;
use crate::safety::{Scope, fingerprint, is_allowed_target};

/// Children of the cache dir that other categories own.
const OTHER_CATEGORIES: [&str; 8] = [
    "thumbnails",
    "crumbs",
    "mozilla",
    "chromium",
    "google-chrome",
    "BraveSoftware",
    "vivaldi",
    "microsoft-edge",
];

/// A home-scoped trash item for `path` (in-system), or `None` when the path
/// is not an allowed target or cannot be fingerprinted.
pub(super) fn new_item(
    cx: &ScanContext,
    category: Category,
    path: PathBuf,
    label: String,
    rebuildable: bool,
) -> Option<BloatItem> {
    if !is_allowed_target(cx.env, cx.cfg, Scope::Home, &path) {
        return None;
    }
    let fp = fingerprint(cx.env, &path).ok()?;
    Some(BloatItem {
        category,
        path: Some(path),
        label,
        size: None,
        rebuildable,
        preselected: true,
        note: None,
        action: ItemAction::Trash,
        scope: Scope::Home,
        fingerprint: Some(fp),
        extra_paths: Vec::new(),
        app_id: None,
    })
}

pub struct CacheScanner;

impl Scanner for CacheScanner {
    fn category(&self) -> Category {
        Category::Cache
    }

    fn scan(&self, cx: &ScanContext) -> Result<Vec<BloatItem>, CoreError> {
        let cache = cx.env.unsys(&cx.env.cache_home);
        let Ok(rd) = fs::read_dir(&cx.env.cache_home) else {
            return Ok(Vec::new());
        };
        // First components (below the cache dir) of the dev-cache paths.
        let dev_paths = dev_cache_paths(cx.env);
        let dev_firsts: Vec<OsString> = dev_paths
            .iter()
            .filter_map(|(_, p)| p.strip_prefix(&cache).ok())
            .filter_map(|rel| rel.components().next())
            .map(|c| c.as_os_str().to_os_string())
            .collect();
        let owners: Vec<(&App, Idents)> = cx.inv.apps.iter().map(|a| (a, Idents::of(a))).collect();

        let mut names: Vec<OsString> = rd.flatten().map(|e| e.file_name()).collect();
        names.sort();
        let mut out = Vec::new();
        for name in names {
            let lossy = name.to_string_lossy().into_owned();
            if OTHER_CATEGORIES.contains(&lossy.as_str()) || dev_firsts.contains(&name) {
                continue;
            }
            let Some(mut item) =
                new_item(cx, Category::Cache, cache.join(&name), lossy.clone(), true)
            else {
                continue;
            };
            let key = NameKey::new(&lossy);
            let mut matching = owners
                .iter()
                .filter(|(_, ids)| matches!(score_name(&key, ids), Some((Confidence::High, _))));
            if let (Some((app, _)), None) = (matching.next(), matching.next()) {
                item.app_id = Some(app.id.clone());
                if !running_processes(cx.env, app).is_empty() {
                    item.preselected = false;
                    item.note = Some(format!("{} is running", app.display_name));
                }
            }
            out.push(item);
        }
        Ok(out)
    }
}

pub struct ThumbnailsScanner;

impl Scanner for ThumbnailsScanner {
    fn category(&self) -> Category {
        Category::Thumbnails
    }

    fn scan(&self, cx: &ScanContext) -> Result<Vec<BloatItem>, CoreError> {
        let real = cx.env.cache_home.join("thumbnails");
        if fs::symlink_metadata(&real).is_err() {
            return Ok(Vec::new());
        }
        let path = cx.env.unsys(&real);
        Ok(
            new_item(cx, Category::Thumbnails, path, "thumbnails".into(), true)
                .into_iter()
                .collect(),
        )
    }
}
