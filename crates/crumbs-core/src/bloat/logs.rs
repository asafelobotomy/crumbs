//! `logs`: old log files under the state dir, and the X session logs.

use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use super::cache::new_item;
use super::{BloatItem, Category, ScanContext, Scanner};
use crate::error::CoreError;

pub struct LogsScanner;

/// Old by the later of mtime and ctime, so a file copied or unpacked
/// with an old mtime is not old.
fn is_old(meta: &fs::Metadata, now: SystemTime, max_age: Duration) -> bool {
    super::temp::last_change(meta)
        .is_some_and(|t| now.duration_since(t).is_ok_and(|age| age > max_age))
}

impl Scanner for LogsScanner {
    fn category(&self) -> Category {
        Category::Logs
    }

    fn scan(&self, cx: &ScanContext) -> Result<Vec<BloatItem>, CoreError> {
        let max_age = Duration::from_secs(cx.cfg.log_age_days.saturating_mul(86_400));
        let mut found: Vec<PathBuf> = Vec::new(); // real paths

        // Iterative walk: no symlinks followed, no mount boundaries crossed.
        if let Ok(root_meta) = fs::symlink_metadata(&cx.env.state_home)
            && root_meta.is_dir()
        {
            let dev = root_meta.dev();
            let mut stack = vec![cx.env.state_home.clone()];
            while let Some(dir) = stack.pop() {
                let Ok(rd) = fs::read_dir(&dir) else { continue };
                for entry in rd.flatten() {
                    let Ok(meta) = entry.metadata() else { continue };
                    let ft = meta.file_type();
                    if ft.is_dir() {
                        if meta.dev() == dev {
                            stack.push(entry.path());
                        }
                    } else if ft.is_file()
                        && entry.path().extension().is_some_and(|e| e == "log")
                        && is_old(&meta, cx.now, max_age)
                    {
                        found.push(entry.path());
                    }
                }
            }
        }
        for name in [".xsession-errors", ".xsession-errors.old"] {
            let p = cx.env.home.join(name);
            if fs::symlink_metadata(&p).is_ok_and(|m| m.is_file() && is_old(&m, cx.now, max_age)) {
                found.push(p);
            }
        }
        found.sort();
        Ok(found
            .into_iter()
            .filter_map(|real| {
                let path = cx.env.unsys(&real);
                let label = crate::env::display_path(cx.env, &path);
                new_item(cx, Category::Logs, path, label, false)
            })
            .collect())
    }
}
