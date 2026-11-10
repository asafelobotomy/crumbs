//! `pkg-cache`: old package versions in pacman's cache. `paccache` prunes
//! to the configured keep count and can report the size in a dry run;
//! without it only `pacman -Sc` is left, which cannot.

use super::super::{BloatItem, Category, ScanContext, Scanner};
use super::{command_failed, not_installed, parse_size, privileged_item};
use crate::command::resolve_trusted;
use crate::error::CoreError;

pub struct PkgCacheScanner;

impl Scanner for PkgCacheScanner {
    fn category(&self) -> Category {
        Category::PkgCache
    }

    fn unavailable(&self, cx: &ScanContext) -> Option<String> {
        super::package_manager(cx.env)
            .is_none()
            .then(|| not_installed(super::NO_PACKAGE_MANAGER))
    }

    fn scan(&self, cx: &ScanContext) -> Result<Vec<BloatItem>, CoreError> {
        match super::package_manager(cx.env) {
            Some(super::PackageManager::AptGet) => return Ok(apt(cx)),
            Some(super::PackageManager::Dnf) => return Ok(dnf(cx)),
            _ => {}
        }
        let Some(paccache) = resolve_trusted(cx.env, "paccache") else {
            let mut item = privileged_item(
                Category::PkgCache,
                "package cache".into(),
                None,
                Some("keeps only installed versions and also removes unused repository sync databases (pacman-contrib not installed)".into()),
                "pacman",
                vec!["-Sc".into(), "--noconfirm".into()],
            );
            item.rebuildable = true;
            return Ok(vec![item]);
        };
        let keep = cx.cfg.pkg_cache_keep;
        let pruned = paccache_saved(cx, &paccache, &format!("-dk{keep}"))?;
        // Packages no longer installed: every cached version can go. A
        // failing dry run only drops this item.
        let uninstalled = paccache_saved(cx, &paccache, "-duk0").ok().flatten();
        let mut items = Vec::new();
        // The uninstalled packages first: they run before the prune.
        for (size, label, args) in [
            (
                uninstalled,
                "cached packages no longer installed".to_string(),
                "-ruk0".to_string(),
            ),
            (
                pruned,
                format!("package cache (keep {keep} versions)"),
                format!("-rk{keep}"),
            ),
        ] {
            let Some(size) = size else { continue };
            let mut item = privileged_item(
                Category::PkgCache,
                label,
                Some(size),
                None,
                "paccache",
                vec![args],
            );
            item.rebuildable = true;
            items.push(item);
        }
        Ok(items)
    }
}

/// What a `paccache` dry run (`-d...`) would free; `None` when there is
/// nothing to prune.
fn paccache_saved(
    cx: &ScanContext,
    paccache: &std::path::Path,
    flags: &str,
) -> Result<Option<u64>, CoreError> {
    let out = cx.runner.run(paccache, &[flags])?;
    if out.status != 0 {
        return Err(command_failed("paccache", &out));
    }
    if out.stdout.contains("no candidate packages found") {
        return Ok(None);
    }
    let size = out
        .stdout
        .lines()
        .rev()
        .find_map(|l| {
            let rest = l.split_once("disk space saved:")?.1;
            parse_size(rest.trim().trim_end_matches(')'))
        })
        .ok_or_else(|| CoreError::Parse {
            what: "paccache output".into(),
            message: "no `disk space saved` line".into(),
        })?;
    Ok((size > 0).then_some(size))
}

/// Debian/Ubuntu: the downloaded `.deb` files `apt-get clean` removes.
fn apt(cx: &ScanContext) -> Vec<BloatItem> {
    let size = files_size(&cx.env.sys("/var/cache/apt/archives"), false, "deb");
    cache_item(size, "apt-get", vec!["clean".into()])
}

/// Fedora: the `.rpm` files `dnf clean packages` removes (dnf5 keeps them
/// under `/var/cache/libdnf5`, dnf4 under `/var/cache/dnf`).
fn dnf(cx: &ScanContext) -> Vec<BloatItem> {
    let dnf5 = cx.env.sys("/var/cache/libdnf5");
    let dir = if dnf5.is_dir() {
        dnf5
    } else {
        cx.env.sys("/var/cache/dnf")
    };
    let size = files_size(&dir, true, "rpm");
    cache_item(size, "dnf", vec!["clean".into(), "packages".into()])
}

fn cache_item(size: u64, program: &str, args: Vec<String>) -> Vec<BloatItem> {
    if size == 0 {
        return Vec::new();
    }
    let mut item = privileged_item(
        Category::PkgCache,
        "package cache".into(),
        Some(size),
        None,
        program,
        args,
    );
    item.rebuildable = true;
    vec![item]
}

/// Total length of the regular files with extension `ext` in `dir`
/// (recursively when `deep`); unreadable entries are skipped, so the size
/// is an estimate.
fn files_size(dir: &std::path::Path, deep: bool, ext: &str) -> u64 {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return 0;
    };
    let mut total = 0;
    for entry in rd.flatten() {
        let Ok(m) = entry.metadata() else { continue };
        let path = entry.path();
        if m.is_dir() && deep {
            total += files_size(&path, deep, ext);
        } else if m.is_file() && path.extension().is_some_and(|e| e == ext) {
            total += m.len();
        }
    }
    total
}
