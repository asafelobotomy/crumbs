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
        resolve_trusted(cx.env, "pacman")
            .is_none()
            .then(|| not_installed("pacman"))
    }

    fn scan(&self, cx: &ScanContext) -> Result<Vec<BloatItem>, CoreError> {
        let Some(paccache) = resolve_trusted(cx.env, "paccache") else {
            let mut item = privileged_item(
                Category::PkgCache,
                "package cache".into(),
                None,
                Some("keeps only installed versions (pacman-contrib not installed)".into()),
                "pacman",
                vec!["-Sc".into(), "--noconfirm".into()],
            );
            item.rebuildable = true;
            return Ok(vec![item]);
        };
        let keep = cx.cfg.pkg_cache_keep;
        let out = cx.runner.run(&paccache, &[&format!("-dk{keep}")])?;
        if out.status != 0 {
            return Err(command_failed("paccache", &out));
        }
        if out.stdout.contains("no candidate packages found") {
            return Ok(Vec::new());
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
        if size == 0 {
            return Ok(Vec::new());
        }
        let mut item = privileged_item(
            Category::PkgCache,
            format!("package cache (keep {keep} versions)"),
            Some(size),
            None,
            "paccache",
            vec![format!("-rk{keep}")],
        );
        item.rebuildable = true;
        Ok(vec![item])
    }
}
