//! `app-cache`: the Chromium caches of Electron and other Chromium-based
//! apps, kept in their folder in `~/.config` rather than in `~/.cache`
//! (Discord, VS Code, Cursor, Slack, Claude, ...). An app is recognised by
//! the `Local State` file Chromium writes at the top of its folder; only the
//! cache folders below are offered, never its settings or site data.

use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use super::cache::new_item;
use super::{BloatItem, Category, ScanContext, Scanner};
use crate::env::Env;
use crate::error::CoreError;

/// Cache folders at the top of a Chromium app's folder: the HTTP cache,
/// compiled scripts, GPU shader caches, and VS Code's `CachedData`.
const LEAVES: [&str; 9] = [
    "Cache",
    "Code Cache",
    "GPUCache",
    "DawnCache",
    "DawnGraphiteCache",
    "DawnWebGPUCache",
    "GrShaderCache",
    "ShaderCache",
    "CachedData",
];

pub struct AppCacheScanner;

impl Scanner for AppCacheScanner {
    fn category(&self) -> Category {
        Category::AppCache
    }

    fn scan(&self, cx: &ScanContext) -> Result<Vec<BloatItem>, CoreError> {
        let config = &cx.env.config_home;
        let Ok(rd) = fs::read_dir(config) else {
            return Ok(Vec::new());
        };
        let mut apps: Vec<PathBuf> = rd
            .flatten()
            .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
            .map(|e| e.path())
            .filter(|d| fs::symlink_metadata(d.join("Local State")).is_ok_and(|m| m.is_file()))
            .collect();
        apps.sort();
        if apps.is_empty() {
            return Ok(Vec::new());
        }
        let live = Live::read(cx.env);
        let mut out = Vec::new();
        for real_app in apps {
            let app_dir = cx.env.unsys(&real_app);
            let name = real_app
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let running = live.uses(&app_dir) || live.uses(&real_app);
            for leaf in LEAVES {
                let real = real_app.join(leaf);
                if !fs::symlink_metadata(&real).is_ok_and(|m| m.is_dir()) {
                    continue;
                }
                let Some(mut item) = new_item(
                    cx,
                    Category::AppCache,
                    app_dir.join(leaf),
                    format!("{name} {leaf}"),
                    true,
                ) else {
                    continue;
                };
                if running {
                    item.preselected = false;
                    item.note = Some(format!("{name} is running"));
                }
                out.push(item);
            }
        }
        Ok(out)
    }
}

/// What running processes point at: the `--user-data-dir=` of each
/// command line, and every open file.
struct Live {
    data_dirs: Vec<PathBuf>,
    open: Vec<PathBuf>,
}

impl Live {
    fn read(env: &Env) -> Live {
        let mut live = Live {
            data_dirs: Vec::new(),
            open: Vec::new(),
        };
        let Ok(rd) = fs::read_dir(env.sys_path(Path::new("/proc"))) else {
            return live;
        };
        for e in rd.flatten() {
            if !e.file_name().as_bytes().iter().all(u8::is_ascii_digit) {
                continue;
            }
            let dir = e.path();
            if let Ok(cmdline) = fs::read(dir.join("cmdline")) {
                live.data_dirs.extend(
                    cmdline
                        .split(|b| *b == 0)
                        .filter_map(|a| a.strip_prefix(b"--user-data-dir="))
                        .map(|p| PathBuf::from(std::ffi::OsStr::from_bytes(p))),
                );
            }
            if let Ok(fds) = fs::read_dir(dir.join("fd")) {
                live.open
                    .extend(fds.flatten().filter_map(|f| fs::read_link(f.path()).ok()));
            }
        }
        live
    }

    /// Whether a process runs with `dir` as its data dir or has a file
    /// open in it.
    fn uses(&self, dir: &Path) -> bool {
        self.data_dirs.iter().any(|d| d == dir) || self.open.iter().any(|f| f.starts_with(dir))
    }
}
