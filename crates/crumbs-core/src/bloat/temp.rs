//! `temp`: the user's old entries directly in `/tmp`. Items are deleted
//! permanently, so anything that might be live is left alone.

use std::ffi::OsStr;
use std::fs::{self, FileType};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use super::{BloatItem, Category, ItemAction, ScanContext, Scanner};
use crate::env::display_path;
use crate::error::CoreError;
use crate::safety::{Scope, fingerprint, is_allowed_target};

/// Name prefixes of live session and service entries (Review Focus #2).
const LIVE_PREFIXES: [&[u8]; 9] = [
    b".X",
    b".ICE-unix",
    b".font-unix",
    b"tmux-",
    b"ssh-",
    b"systemd-private-",
    b"pulse-",
    b"krb5cc",
    b".s.PGSQL",
];

/// Most entries walked inside one directory; a bigger one is skipped.
const WALK_CAP: usize = 10_000;

const NOTE: &str = "deleted permanently (temporary file)";

fn is_old(mtime: SystemTime, now: SystemTime, max_age: Duration) -> bool {
    now.duration_since(mtime).is_ok_and(|age| age > max_age)
}

/// Whether a direct child of `/tmp` may be listed, judged from its own
/// lstat data: owned by the user, a regular file or directory (never a
/// symlink, socket, fifo or device), no live-entry name prefix, and older
/// than `max_age`. A directory's contents are checked separately.
pub(crate) fn temp_candidate(
    name: &OsStr,
    owner_uid: u32,
    user_uid: u32,
    file_type: FileType,
    mtime: SystemTime,
    now: SystemTime,
    max_age: Duration,
) -> bool {
    let bytes = name.as_bytes();
    owner_uid == user_uid
        && (file_type.is_file() || file_type.is_dir())
        && !LIVE_PREFIXES.iter().any(|p| bytes.starts_with(p))
        && is_old(mtime, now, max_age)
}

/// Whether everything below `dir` (real path) is old, owned by the user,
/// a file, directory or symlink, and on device `dev`, within [`WALK_CAP`]
/// entries. Never follows symlinks; any read error fails closed.
fn contents_old(dir: &Path, dev: u64, user_uid: u32, now: SystemTime, max_age: Duration) -> bool {
    let mut seen = 0usize;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = fs::read_dir(&d) else {
            return false;
        };
        for entry in rd {
            let Ok(entry) = entry else { return false };
            seen += 1;
            if seen > WALK_CAP {
                return false;
            }
            let path = entry.path();
            let Ok(meta) = fs::symlink_metadata(&path) else {
                return false;
            };
            let ft = meta.file_type();
            if meta.uid() != user_uid
                || meta.dev() != dev
                || !(ft.is_file() || ft.is_dir() || ft.is_symlink())
                || !meta.modified().is_ok_and(|m| is_old(m, now, max_age))
            {
                return false;
            }
            if ft.is_dir() {
                stack.push(path);
            }
        }
    }
    true
}

pub struct TempScanner;

impl Scanner for TempScanner {
    fn category(&self) -> Category {
        Category::Temp
    }

    fn scan(&self, cx: &ScanContext) -> Result<Vec<BloatItem>, CoreError> {
        let tmp_real = cx.env.sys("/tmp");
        // `/tmp` itself must be a real directory; verify refuses otherwise.
        let Ok(tmp_meta) = fs::symlink_metadata(&tmp_real) else {
            return Ok(Vec::new());
        };
        if !tmp_meta.file_type().is_dir() {
            return Ok(Vec::new());
        }
        let Ok(rd) = fs::read_dir(&tmp_real) else {
            return Ok(Vec::new());
        };
        let max_age = Duration::from_secs(cx.cfg.temp_age_days.saturating_mul(86_400));
        let mut names: Vec<_> = rd.flatten().map(|e| e.file_name()).collect();
        names.sort();

        let mut out = Vec::new();
        for name in names {
            let real = tmp_real.join(&name);
            let Ok(meta) = fs::symlink_metadata(&real) else {
                continue;
            };
            let Ok(mtime) = meta.modified() else { continue };
            let ft = meta.file_type();
            if !temp_candidate(&name, meta.uid(), cx.env.uid, ft, mtime, cx.now, max_age) {
                continue;
            }
            if ft.is_dir()
                && (meta.dev() != tmp_meta.dev()
                    || !contents_old(&real, meta.dev(), cx.env.uid, cx.now, max_age))
            {
                continue;
            }
            let path = PathBuf::from("/tmp").join(&name);
            if !is_allowed_target(cx.env, cx.cfg, Scope::UserTemp, &path) {
                continue;
            }
            let Ok(fp) = fingerprint(cx.env, &path) else {
                continue;
            };
            out.push(BloatItem {
                category: Category::Temp,
                label: display_path(cx.env, &path),
                path: Some(path),
                size: None,
                rebuildable: false,
                preselected: true,
                note: Some(NOTE.into()),
                action: ItemAction::Delete,
                scope: Scope::UserTemp,
                fingerprint: Some(fp),
                extra_paths: Vec::new(),
                app_id: None,
            });
        }
        Ok(out)
    }
}
