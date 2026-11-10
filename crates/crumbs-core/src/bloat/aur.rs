//! `aur-build`: the build trees pamac (Manjaro's package manager) keeps in
//! `<BuildDirectory>/pamac-build-<user>`, one item for the whole tree. pamac
//! builds an AUR package there and leaves the sources and build output
//! behind; it rebuilds them when it updates the package. yay's and paru's
//! clones live in `~/.cache` and are listed under `cache`.

use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use super::{BloatItem, Category, ItemAction, ScanContext, Scanner};
use crate::error::CoreError;
use crate::executor::{process_using, process_working_in};
use crate::safety::{Scope, fingerprint, is_allowed_target};

/// pamac's default `BuildDirectory`.
const DEFAULT_BUILD_DIR: &str = "/var/tmp";

const NOTE: &str = "deleted permanently; pamac rebuilds a package when it updates it";

pub struct AurBuildScanner;

impl Scanner for AurBuildScanner {
    fn category(&self) -> Category {
        Category::AurBuild
    }

    fn scan(&self, cx: &ScanContext) -> Result<Vec<BloatItem>, CoreError> {
        let build_dir = build_directory(cx);
        // Only where the safety scopes allow a user's own tree.
        if !matches!(build_dir.to_str(), Some("/tmp" | "/var/tmp")) {
            return Ok(Vec::new());
        }
        let Ok(rd) = fs::read_dir(cx.env.sys_path(&build_dir)) else {
            return Ok(Vec::new());
        };
        let mut out = Vec::new();
        for entry in rd.flatten() {
            let name = entry.file_name();
            if !name.to_string_lossy().starts_with("pamac-build-") {
                continue;
            }
            let Ok(meta) = fs::symlink_metadata(entry.path()) else {
                continue;
            };
            if !meta.is_dir() || meta.uid() != cx.env.uid {
                continue;
            }
            let path = build_dir.join(&name);
            if !is_allowed_target(cx.env, cx.cfg, Scope::UserTemp, &path) {
                continue;
            }
            let Ok(fp) = fingerprint(cx.env, &path) else {
                continue;
            };
            let packages = fs::read_dir(entry.path())
                .map(|rd| {
                    rd.flatten()
                        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
                        .count()
                })
                .unwrap_or(0);
            let what = if packages == 1 { "package" } else { "packages" };
            let mut item = BloatItem {
                category: Category::AurBuild,
                path: Some(path.clone()),
                label: format!("pamac build trees ({packages} {what})"),
                size: None,
                rebuildable: true,
                preselected: true,
                note: Some(NOTE.into()),
                action: ItemAction::Delete,
                scope: Scope::UserTemp,
                fingerprint: Some(fp),
                extra_paths: Vec::new(),
                app_id: None,
            };
            if let Some((pid, comm)) =
                process_working_in(cx.env, &path).or_else(|| process_using(cx.env, &path))
            {
                item.preselected = false;
                item.note = Some(format!("in use by {comm} (pid {pid})"));
            }
            out.push(item);
        }
        out.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(out)
    }
}

/// pamac's `BuildDirectory` from `/etc/pamac.conf`, or its default.
fn build_directory(cx: &ScanContext) -> PathBuf {
    let text =
        fs::read_to_string(cx.env.sys_path(Path::new("/etc/pamac.conf"))).unwrap_or_default();
    text.lines()
        .filter_map(|l| l.trim().strip_prefix("BuildDirectory"))
        .filter_map(|rest| rest.trim().strip_prefix('='))
        .map(|v| v.trim().trim_end_matches('/'))
        .find(|v| v.starts_with('/'))
        .map_or_else(|| PathBuf::from(DEFAULT_BUILD_DIR), PathBuf::from)
}
