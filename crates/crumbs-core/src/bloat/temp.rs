//! `temp`: the user's old entries directly in `/tmp`. Items are deleted
//! permanently, so anything that might be live is left alone.

use std::collections::HashSet;
use std::ffi::{OsStr, OsString};
use std::fs::{self, FileType, Metadata};
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use super::{BloatItem, Category, ItemAction, ScanContext, Scanner};
use crate::config::Config;
use crate::env::{Env, display_path};
use crate::error::CoreError;
use crate::safety::{Scope, fingerprint, is_allowed_target, is_safe_tmp};

/// Name prefixes of live session and service entries (Review Focus #2,
/// extended by controller ruling R2).
const LIVE_PREFIXES: [&[u8]; 24] = [
    b".X",
    b".ICE-unix",
    b".font-unix",
    b"tmux-",
    b"ssh-",
    b"systemd-private-",
    b"pulse-",
    b"krb5cc",
    b".s.PGSQL",
    b"xauth",
    b".xauth",
    b"serverauth",
    b"sddm",
    b"gdm",
    b"lightdm",
    b"kde",
    b"plasma",
    b"dbus",
    b".wine",
    b"nvidia",
    b"mozilla",
    b"vscode",
    b"pyright",
    b"jetbrains",
];

/// Most entries walked inside one directory; a bigger one is skipped.
const WALK_CAP: usize = 10_000;

const NOTE: &str = "deleted permanently (temporary file)";

/// The scanner's error when run as root (review finding I1).
pub(crate) const ROOT_REFUSAL: &str = "temp cleanup is not available as root";

/// The lstat data the candidate rule looks at.
#[derive(Clone, Copy)]
pub(crate) struct EntryStat {
    pub owner_uid: u32,
    pub file_type: FileType,
    /// The later of mtime and ctime: archive extraction keeps old mtimes,
    /// but the ctime says when the entry landed here.
    pub changed: SystemTime,
}

impl EntryStat {
    pub(crate) fn of(m: &Metadata) -> Option<Self> {
        Some(EntryStat {
            owner_uid: m.uid(),
            file_type: m.file_type(),
            changed: last_change(m)?,
        })
    }
}

/// `max(mtime, ctime)` of an lstat result.
pub(crate) fn last_change(m: &Metadata) -> Option<SystemTime> {
    let mtime = m.modified().ok()?;
    let secs = u64::try_from(m.ctime()).ok()?;
    let nanos = u32::try_from(m.ctime_nsec()).ok()?;
    let ctime = SystemTime::UNIX_EPOCH.checked_add(Duration::new(secs, nanos))?;
    Some(mtime.max(ctime))
}

fn is_old(t: SystemTime, now: SystemTime, max_age: Duration) -> bool {
    now.duration_since(t).is_ok_and(|age| age > max_age)
}

/// Whether a direct child of `/tmp` may be listed, judged from its own
/// lstat data: owned by the user, a regular file or directory (never a
/// symlink, socket, fifo or device), no live-entry name prefix, and
/// neither modified nor changed (mtime, ctime) within `max_age`. A directory's contents
/// are checked separately.
pub(crate) fn temp_candidate(
    name: &OsStr,
    st: &EntryStat,
    user_uid: u32,
    now: SystemTime,
    max_age: Duration,
) -> bool {
    let bytes = name.as_bytes();
    st.owner_uid == user_uid
        && (st.file_type.is_file() || st.file_type.is_dir())
        && !LIVE_PREFIXES.iter().any(|p| bytes.starts_with(p))
        && is_old(st.changed, now, max_age)
}

/// `(dev, ino)` of every file a process has open, mapped, or as its
/// working directory or executable, from `/proc` (under the env root).
/// Processes that can't be read are skipped.
fn open_inodes(env: &Env) -> HashSet<(u64, u64)> {
    let mut set = HashSet::new();
    let Ok(procs) = fs::read_dir(env.sys_path(Path::new("/proc"))) else {
        return set;
    };
    for p in procs.flatten() {
        let name = p.file_name();
        if name.is_empty() || !name.as_bytes().iter().all(u8::is_ascii_digit) {
            continue;
        }
        let dir = p.path();
        let mut links: Vec<PathBuf> = vec![dir.join("cwd"), dir.join("exe")];
        if let Ok(fds) = fs::read_dir(dir.join("fd")) {
            links.extend(fds.flatten().map(|e| e.path()));
        }
        // Stat through the link: the open file itself.
        for link in links {
            if let Ok(m) = fs::metadata(&link) {
                set.insert((m.dev(), m.ino()));
            }
        }
        if let Ok(maps) = fs::read_to_string(dir.join("maps")) {
            set.extend(maps.lines().filter_map(parse_maps_line));
        }
    }
    set
}

/// `(dev, ino)` of a file-backed `/proc/<pid>/maps` line.
fn parse_maps_line(line: &str) -> Option<(u64, u64)> {
    let mut f = line.split_ascii_whitespace();
    let dev = f.nth(3)?;
    let ino: u64 = f.next()?.parse().ok()?;
    if ino == 0 {
        return None;
    }
    let (maj, min) = dev.split_once(':')?;
    let maj = u32::from_str_radix(maj, 16).ok()?;
    let min = u32::from_str_radix(min, 16).ok()?;
    Some((rustix::fs::makedev(maj, min), ino))
}

/// Names in `dir` (real path).
fn list_dir(dir: &Path) -> io::Result<Vec<OsString>> {
    fs::read_dir(dir)?
        .map(|e| e.map(|e| e.file_name()))
        .collect()
}

/// What the stale checks need besides the entry itself.
struct Ctx<'a> {
    user_uid: u32,
    tmp_dev: u64,
    now: SystemTime,
    max_age: Duration,
    open: &'a HashSet<(u64, u64)>,
}

/// Whether everything below `dir` (real path) is old (by max(mtime, ctime)), owned by the user, a file, directory or symlink, on device
/// `dev`, and not open in any process, within [`WALK_CAP`] entries. Never
/// follows symlinks; any read error fails closed.
fn contents_old(dir: &Path, dev: u64, cx: &Ctx) -> bool {
    let mut seen = 0usize;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(names) = list_dir(&d) else {
            return false;
        };
        for name in names {
            seen += 1;
            if seen > WALK_CAP {
                return false;
            }
            let path = d.join(name);
            let Ok(meta) = fs::symlink_metadata(&path) else {
                return false;
            };
            let Some(st) = EntryStat::of(&meta) else {
                return false;
            };
            let ft = st.file_type;
            if st.owner_uid != cx.user_uid
                || meta.dev() != dev
                || !(ft.is_file() || ft.is_dir() || ft.is_symlink())
                || !is_old(st.changed, cx.now, cx.max_age)
                || cx.open.contains(&(meta.dev(), meta.ino()))
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

/// Why the `/tmp` child `name` (real path `real`) must not be removed now,
/// or `Ok` when it is a stale temporary entry.
fn stale_check(real: &Path, name: &OsStr, cx: &Ctx) -> Result<(), &'static str> {
    const NOT_STALE: &str = "is no longer an old temporary file";
    let meta = fs::symlink_metadata(real).map_err(|_| NOT_STALE)?;
    let st = EntryStat::of(&meta).ok_or(NOT_STALE)?;
    if !temp_candidate(name, &st, cx.user_uid, cx.now, cx.max_age) {
        return Err(NOT_STALE);
    }
    if cx.open.contains(&(meta.dev(), meta.ino())) {
        return Err("is open in a running process");
    }
    if st.file_type.is_dir() && (meta.dev() != cx.tmp_dev || !contents_old(real, meta.dev(), cx)) {
        return Err(NOT_STALE);
    }
    Ok(())
}

fn max_age(cfg: &Config) -> Duration {
    Duration::from_secs(cfg.temp_age_days.saturating_mul(86_400))
}

/// The scanner's error when `/tmp` is not root-owned and sticky.
pub(crate) const UNSAFE_TMP: &str = "unsafe /tmp: not root-owned with sticky bit";

/// `/tmp` (real path) and its device, when it is a real directory; `Err`
/// when it is one but not root-owned with the sticky bit.
fn tmp_dir(env: &Env) -> Result<Option<(PathBuf, u64)>, &'static str> {
    let real = env.sys("/tmp");
    let Ok(m) = fs::symlink_metadata(&real) else {
        return Ok(None);
    };
    if !m.file_type().is_dir() {
        return Ok(None);
    }
    if !is_safe_tmp(env, &m) {
        return Err(UNSAFE_TMP);
    }
    Ok(Some((real, m.dev())))
}

/// Re-runs the scan's rules for one `/tmp` child (in-system `path`) right
/// before it is deleted: still a stale candidate, its contents still old,
/// and nothing in it open. `Err` holds the reason, to follow the path in
/// `refusing: <path> <reason>`.
pub(crate) fn still_stale(
    env: &Env,
    cfg: &Config,
    path: &Path,
    now: SystemTime,
) -> Result<(), String> {
    let not_temp = || "is not a direct child of /tmp".to_string();
    if path.parent() != Some(Path::new("/tmp")) {
        return Err(not_temp());
    }
    let name = path.file_name().ok_or_else(not_temp)?;
    let (tmp_real, tmp_dev) = tmp_dir(env)?.ok_or_else(not_temp)?;
    let open = open_inodes(env);
    let cx = Ctx {
        user_uid: env.uid,
        tmp_dev,
        now,
        max_age: max_age(cfg),
        open: &open,
    };
    stale_check(&tmp_real.join(name), name, &cx).map_err(str::to_string)
}

pub struct TempScanner;

impl Scanner for TempScanner {
    fn category(&self) -> Category {
        Category::Temp
    }

    /// Root could remove other users' files; temp cleanup is per user, so
    /// it is not an error when crumbs runs as root, just not offered.
    fn unavailable(&self, cx: &ScanContext) -> Option<String> {
        (cx.env.uid == 0).then(|| "not available as root".to_string())
    }

    fn scan(&self, cx: &ScanContext) -> Result<Vec<BloatItem>, CoreError> {
        if cx.env.uid == 0 {
            return Err(CoreError::Unavailable(ROOT_REFUSAL));
        }
        // `/tmp` itself must be a real, root-owned, sticky directory;
        // verify refuses otherwise.
        let Some((tmp_real, tmp_dev)) = tmp_dir(cx.env).map_err(CoreError::Unavailable)? else {
            return Ok(Vec::new());
        };
        let Ok(mut names) = list_dir(&tmp_real) else {
            return Ok(Vec::new());
        };
        names.sort();
        let open = open_inodes(cx.env);
        let rules = Ctx {
            user_uid: cx.env.uid,
            tmp_dev,
            now: cx.now,
            max_age: max_age(cx.cfg),
            open: &open,
        };

        let mut out = Vec::new();
        for name in names {
            if stale_check(&tmp_real.join(&name), &name, &rules).is_err() {
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

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use std::time::UNIX_EPOCH;

    #[test]
    fn maps_lines() {
        let dev = rustix::fs::makedev(0x103, 0x2);
        assert_eq!(
            parse_maps_line("7f00-7f10 r-xp 00000000 103:02 4242 /usr/lib/libc.so.6"),
            Some((dev, 4242))
        );
        assert_eq!(
            parse_maps_line("7f00-7f10 rw-p 00000000 00:00 0 [heap]"),
            None
        );
        assert_eq!(parse_maps_line("garbage"), None);
    }

    #[test]
    fn epoch_is_old() {
        assert!(is_old(
            UNIX_EPOCH,
            SystemTime::now(),
            Duration::from_secs(1)
        ));
    }
}
