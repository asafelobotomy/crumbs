//! `browser-cache`: per-profile cache dirs of the common browsers, native
//! and Flatpak.

use std::collections::HashSet;
use std::ffi::OsString;
use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use super::cache::new_item;
use super::{BloatItem, Category, ScanContext, Scanner};
use crate::command::resolve_trusted;
use crate::env::Env;
use crate::error::CoreError;

/// One path segment of a cache pattern.
#[derive(Clone, Copy)]
enum Seg {
    Lit(&'static str),
    /// `*`: one existing real directory, part of the profile name.
    Any,
}

const CHROMIUM_LEAVES: &[&str] = &["Cache", "Code Cache", "GPUCache"];

struct Browser {
    name: &'static str,
    /// Below the cache dir; every match is a profile dir.
    profile: &'static [Seg],
    /// Cache dirs inside a profile dir.
    leaves: &'static [&'static str],
    /// Process exe basenames that mean the browser is running.
    exes: &'static [&'static str],
    flatpak_id: &'static str,
}

const BROWSERS: [Browser; 6] = [
    Browser {
        name: "Firefox",
        profile: &[Seg::Lit("mozilla"), Seg::Lit("firefox"), Seg::Any],
        leaves: &["cache2"],
        exes: &["firefox", "firefox-bin"],
        flatpak_id: "org.mozilla.firefox",
    },
    Browser {
        name: "Chromium",
        profile: &[Seg::Lit("chromium"), Seg::Any],
        leaves: CHROMIUM_LEAVES,
        exes: &["chromium"],
        flatpak_id: "org.chromium.Chromium",
    },
    Browser {
        name: "Chrome",
        profile: &[Seg::Lit("google-chrome"), Seg::Any],
        leaves: CHROMIUM_LEAVES,
        exes: &["chrome"],
        flatpak_id: "com.google.Chrome",
    },
    Browser {
        name: "Brave",
        profile: &[Seg::Lit("BraveSoftware"), Seg::Any, Seg::Any],
        leaves: CHROMIUM_LEAVES,
        exes: &["brave"],
        flatpak_id: "com.brave.Browser",
    },
    Browser {
        name: "Vivaldi",
        profile: &[Seg::Lit("vivaldi"), Seg::Any],
        leaves: CHROMIUM_LEAVES,
        exes: &["vivaldi-bin"],
        flatpak_id: "com.vivaldi.Vivaldi",
    },
    Browser {
        name: "Edge",
        profile: &[Seg::Lit("microsoft-edge"), Seg::Any],
        leaves: CHROMIUM_LEAVES,
        exes: &["msedge"],
        flatpak_id: "com.microsoft.Edge",
    },
];

/// Exists and is a real directory (lstat: a symlink is not).
fn is_real_dir(p: &Path) -> bool {
    fs::symlink_metadata(p).is_ok_and(|m| m.file_type().is_dir())
}

/// Every directory from home (exclusive) down to `base` (real paths) is a
/// real directory. `false` for a base outside home.
fn real_chain_from_home(env: &Env, base: &Path) -> bool {
    let Ok(rel) = base.strip_prefix(&env.home) else {
        return false;
    };
    let mut dir = env.home.clone();
    for comp in rel.components() {
        dir.push(comp.as_os_str());
        if !is_real_dir(&dir) {
            return false;
        }
    }
    true
}

/// Expands `segs` below `base` one segment at a time over existing real
/// directories. Returns each match with the names its `Any` segments took.
fn expand(base: &Path, segs: &[Seg]) -> Vec<(PathBuf, Vec<OsString>)> {
    let mut found = vec![(base.to_path_buf(), Vec::new())];
    for seg in segs {
        let mut next = Vec::new();
        for (dir, parts) in found {
            match seg {
                Seg::Lit(name) => {
                    let p = dir.join(name);
                    if is_real_dir(&p) {
                        next.push((p, parts));
                    }
                }
                Seg::Any => {
                    let Ok(rd) = fs::read_dir(&dir) else { continue };
                    let mut names: Vec<OsString> = rd.flatten().map(|e| e.file_name()).collect();
                    names.sort();
                    for name in names {
                        let p = dir.join(&name);
                        if is_real_dir(&p) {
                            let mut parts = parts.clone();
                            parts.push(name);
                            next.push((p, parts));
                        }
                    }
                }
            }
        }
        found = next;
    }
    found
}

/// Basenames of every running process's exe, with the ` (deleted)` suffix
/// of a replaced binary removed.
fn running_exes(env: &Env) -> HashSet<Vec<u8>> {
    let Ok(rd) = fs::read_dir(env.sys_path(Path::new("/proc"))) else {
        return HashSet::new();
    };
    rd.flatten()
        .filter(|e| {
            let n = e.file_name();
            !n.is_empty() && n.as_bytes().iter().all(u8::is_ascii_digit)
        })
        .filter_map(|e| fs::read_link(e.path().join("exe")).ok())
        .filter_map(|target| {
            let base = target.file_name()?.as_bytes();
            Some(base.strip_suffix(b" (deleted)").unwrap_or(base).to_vec())
        })
        .collect()
}

/// App IDs `flatpak ps` lists, or `None` when it can't be asked.
fn flatpak_running(cx: &ScanContext) -> Option<HashSet<String>> {
    let flatpak = resolve_trusted(cx.env, "flatpak")?;
    let out = cx
        .runner
        .run(&flatpak, &["ps", "--columns=application"])
        .ok()?;
    if out.status != 0 {
        return None;
    }
    Some(out.stdout.lines().map(|l| l.trim().to_string()).collect())
}

pub struct BrowserCacheScanner;

impl Scanner for BrowserCacheScanner {
    fn category(&self) -> Category {
        Category::BrowserCache
    }

    fn scan(&self, cx: &ScanContext) -> Result<Vec<BloatItem>, CoreError> {
        let exes = running_exes(cx.env);
        let mut flatpak_ps: Option<Option<HashSet<String>>> = None;
        let mut out = Vec::new();
        for b in &BROWSERS {
            let native_running = b.exes.iter().any(|e| exes.contains(e.as_bytes()));
            let flatpak_base = cx
                .env
                .home
                .join(".var/app")
                .join(b.flatpak_id)
                .join("cache");
            for (base, is_flatpak) in [(cx.env.cache_home.clone(), false), (flatpak_base, true)] {
                if !real_chain_from_home(cx.env, &base) {
                    continue;
                }
                for (profile_dir, parts) in expand(&base, b.profile) {
                    let profile = parts
                        .iter()
                        .map(|p| p.to_string_lossy())
                        .collect::<Vec<_>>()
                        .join("/");
                    let label = format!("{} ({profile})", b.name);
                    for leaf in b.leaves {
                        let real = profile_dir.join(leaf);
                        if !is_real_dir(&real) {
                            continue;
                        }
                        let path = cx.env.unsys(&real);
                        let Some(mut item) =
                            new_item(cx, Category::BrowserCache, path, label.clone(), true)
                        else {
                            continue;
                        };
                        let note = if native_running {
                            Some(format!("{} is running", b.name))
                        } else if is_flatpak {
                            match flatpak_ps.get_or_insert_with(|| flatpak_running(cx)) {
                                Some(ids) if ids.contains(b.flatpak_id) => {
                                    Some(format!("{} is running", b.name))
                                }
                                Some(_) => None,
                                None => {
                                    Some(format!("could not check whether {} is running", b.name))
                                }
                            }
                        } else {
                            None
                        };
                        if note.is_some() {
                            item.preselected = false;
                            item.note = note;
                        }
                        out.push(item);
                    }
                }
            }
        }
        Ok(out)
    }
}
