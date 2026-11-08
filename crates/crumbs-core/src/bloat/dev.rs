//! `dev-cache`: build-tool caches that the tool recreates on demand.

use std::fs;
use std::path::PathBuf;

use super::{BloatItem, Category, ScanContext, Scanner, cache::new_item};
use crate::env::Env;
use crate::error::CoreError;

/// `(tool, path relative to home)`.
const DEV_CACHES: [(&str, &str); 14] = [
    ("cargo", ".cargo/registry/cache"),
    ("cargo", ".cargo/registry/src"),
    ("cargo", ".cargo/git/checkouts"),
    ("npm", ".npm/_cacache"),
    ("pnpm", ".local/share/pnpm/store"),
    ("pnpm", ".cache/pnpm"),
    ("yarn", ".cache/yarn"),
    ("pip", ".cache/pip"),
    ("go", ".cache/go-build"),
    ("go", "go/pkg/mod/cache"),
    ("gradle", ".gradle/caches"),
    ("maven", ".m2/repository"),
    ("ccache", ".cache/ccache"),
    ("ccache", ".ccache"),
];

/// Every table entry as `(tool, in-system path)`, whether or not it exists.
pub(super) fn dev_cache_paths(env: &Env) -> Vec<(&'static str, PathBuf)> {
    let home = env.unsys(&env.home);
    DEV_CACHES
        .iter()
        .map(|(tool, rel)| (*tool, home.join(rel)))
        .collect()
}

pub struct DevCacheScanner;

impl Scanner for DevCacheScanner {
    fn category(&self) -> Category {
        Category::DevCache
    }

    fn scan(&self, cx: &ScanContext) -> Result<Vec<BloatItem>, CoreError> {
        Ok(dev_cache_paths(cx.env)
            .into_iter()
            .filter(|(_, p)| fs::symlink_metadata(cx.env.sys_path(p)).is_ok())
            .filter_map(|(tool, p)| new_item(cx, Category::DevCache, p, tool.to_string(), true))
            .collect())
    }
}
