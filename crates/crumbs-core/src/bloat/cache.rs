//! `cache` and `thumbnails`: the user's cache directory, one item per app.

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

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

        // Only files, directories and symlinks: a socket, fifo or device
        // is live or special, never cache to trash.
        let mut names: Vec<OsString> = rd
            .flatten()
            .filter(|e| {
                e.file_type()
                    .is_ok_and(|t| t.is_file() || t.is_dir() || t.is_symlink())
            })
            .map(|e| e.file_name())
            .collect();
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
            let matching: Vec<&App> = owners
                .iter()
                .filter(|(_, ids)| matches!(score_name(&key, ids), Some((Confidence::High, _))))
                .map(|(app, _)| *app)
                .collect();
            if let [app] = matching.as_slice() {
                item.app_id = Some(app.id.clone());
            }
            if let Some(app) = matching
                .iter()
                .find(|a| !running_processes(cx.env, a).is_empty())
            {
                item.preselected = false;
                item.note = Some(format!("{} is running", app.display_name));
            }
            let real = cx.env.cache_home.join(&name);
            let git = if known_git_cache(&lossy) || tagged_cache(&real) {
                None
            } else {
                git_checkout(&real)
            };
            if let Some(inner) = git {
                // Someone's work, not a cache: never rebuilt by any tool.
                item.preselected = false;
                item.rebuildable = false;
                item.note = Some(format!("contains a git checkout ({})", inner.display()));
            } else if let Some((pid, comm)) =
                crate::executor::process_working_in(cx.env, &cache.join(&name))
            {
                item.preselected = false;
                item.note = Some(format!("in use by {comm} (pid {pid})"));
            }
            out.push(item);
        }
        Ok(out)
    }
}

/// Caches made of git checkouts by design: AUR helpers' build clones,
/// pre-commit's hook repositories, antidote's plugins. Rebuilt on demand.
const KNOWN_GIT_CACHES: [&str; 5] = ["yay", "paru", "pikaur", "pre-commit", "antidote"];

fn known_git_cache(name: &str) -> bool {
    KNOWN_GIT_CACHES.contains(&name)
}

/// Whether the real directory `dir` is tagged as a cache: a `CACHEDIR.TAG`
/// at its root with the standard signature (bford.info/cachedir).
fn tagged_cache(dir: &Path) -> bool {
    const SIGNATURE: &[u8] = b"Signature: 8a477f597d28d172789f06886806bc55";
    fs::read(dir.join("CACHEDIR.TAG")).is_ok_and(|b| b.starts_with(SIGNATURE))
}

/// Levels below a cache item searched for a git working tree: the item
/// itself and two more (`~/.cache/<work>/<repo>/.git`).
const GIT_DEPTH: usize = 2;
/// At most this many directories are looked at per item, so a huge cache
/// (thousands of hashed directories) stays cheap.
const GIT_DIR_LIMIT: usize = 20_000;

/// The first git working tree in the real directory `dir` or up to
/// [`GIT_DEPTH`] levels below it, relative to `dir` (`.` for `dir`
/// itself). A working tree has a `.git` directory holding an `index`, or
/// a `.git` file (a linked worktree); a tool's own clone or bare
/// repository has neither. Symlinks are not followed.
fn git_checkout(dir: &Path) -> Option<PathBuf> {
    // A symlinked item: only the link would go.
    if fs::symlink_metadata(dir).map_or(true, |m| !m.is_dir()) {
        return None;
    }
    let is_tree = |d: &Path| {
        let git = d.join(".git");
        match fs::symlink_metadata(&git) {
            Ok(m) if m.is_dir() => {
                fs::symlink_metadata(git.join("index")).is_ok_and(|i| i.is_file())
            }
            Ok(m) if m.is_file() => fs::read(&git).is_ok_and(|b| b.starts_with(b"gitdir:")),
            _ => false,
        }
    };
    let mut level: Vec<PathBuf> = vec![dir.to_path_buf()];
    let mut seen = 0usize;
    for depth in 0..=GIT_DEPTH {
        let mut next = Vec::new();
        for d in &level {
            seen += 1;
            if seen > GIT_DIR_LIMIT {
                return None;
            }
            if is_tree(d) {
                let rel = d.strip_prefix(dir).unwrap_or(d);
                return Some(if rel.as_os_str().is_empty() {
                    PathBuf::from(".")
                } else {
                    rel.to_path_buf()
                });
            }
            if depth == GIT_DEPTH {
                continue;
            }
            let Ok(rd) = fs::read_dir(d) else { continue };
            next.extend(
                rd.flatten()
                    .filter(|e| e.file_name() != ".git" && e.file_type().is_ok_and(|t| t.is_dir()))
                    .map(|e| e.path()),
            );
        }
        level = next;
    }
    None
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
