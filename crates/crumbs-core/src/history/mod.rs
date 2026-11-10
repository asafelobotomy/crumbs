use std::fs::{self, DirBuilder, OpenOptions};
use std::io::{self, Write};
use std::os::fd::{AsFd, AsRawFd, OwnedFd};
use std::os::unix::fs::{DirBuilderExt, FileExt, OpenOptionsExt};

use rustix::fs::{FileType, Mode as RawMode, OFlags};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::env::Env;
use crate::error::CoreError;
use crate::safety::Scope;
use crate::trash::Trashed;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum HistoryAction {
    Trash,
    Delete,
    /// `edit-file`, like the plan's action tag. Lines written before that
    /// spelling (`edit_file`) still parse.
    #[serde(alias = "edit_file")]
    EditFile,
    Restore,
    /// A `UserCommand` ran; it cannot be undone.
    Command,
    /// A `PackageRemove` ran as root; `packages` says what it removed.
    /// Packages cannot be restored by crumbs.
    PackageRemove,
    /// A `PrivilegedCommand` ran as root; it cannot be undone.
    PrivilegedCommand,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub id: String,
    pub batch: String,
    pub time: String,
    pub action: HistoryAction,
    pub path: PathBuf,
    pub trashed: Option<Trashed>,
    pub backup: Option<PathBuf>,
    pub size: u64,
    pub ok: bool,
    pub error: Option<String>,
    pub app_id: Option<String>,
    /// `Restore` entries only: the id of the entry this one restored.
    #[serde(default)]
    pub restores: Option<String>,
    /// `Command`, `PrivilegedCommand` and `PackageRemove` entries only: the
    /// program's arguments.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub argv: Option<Vec<String>>,
    /// `PackageRemove` entries only: each package's `(name, version)`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub packages: Option<Vec<(String, String)>>,
    /// `PackageRemove` entries only: the package source. Missing in lines
    /// written before it existed, which were all pacman.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<crate::model::SourceKind>,
    /// The command whose batch this is: [`KIND_UNINSTALL`],
    /// [`KIND_ORPHANS`] or [`KIND_CLEAN`]. Missing in lines written before
    /// it existed, and in `Restore` entries.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// `Delete` entries only: the scope the target was deleted in, when it
    /// is not [`Scope::Home`]. Missing in lines written before it existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<Scope>,
}

pub const KIND_UNINSTALL: &str = "uninstall";
pub const KIND_ORPHANS: &str = "orphans";
pub const KIND_CLEAN: &str = "clean";

fn io_err(path: &Path, source: io::Error) -> CoreError {
    CoreError::Io {
        path: path.to_path_buf(),
        source,
    }
}

/// Current unix milliseconds as a decimal string; entry ids are `<batch>-<index>`.
pub fn new_batch_id() -> String {
    jiff::Timestamp::now().as_millisecond().to_string()
}

/// A batch id greater than every numeric batch already in the history, so
/// two runs within the same millisecond never share ids. An unreadable
/// history falls back to [`new_batch_id`].
pub fn next_batch_id(env: &Env) -> String {
    let now = jiff::Timestamp::now().as_millisecond();
    let now = u128::try_from(now).unwrap_or(0);
    let last = read_history(env)
        .map(|(entries, _)| {
            entries
                .iter()
                .filter_map(|e| e.batch.parse::<u128>().ok())
                .max()
        })
        .unwrap_or(None);
    match last {
        Some(l) if l >= now => l.saturating_add(1).to_string(),
        _ => now.to_string(),
    }
}

/// RFC 3339 timestamp for `HistoryEntry::time`.
pub fn now_rfc3339() -> String {
    jiff::Timestamp::now().to_string()
}

/// `t` as an RFC 3339 UTC timestamp in whole seconds (any fraction is
/// dropped), or `None` for a time outside the range of timestamps.
pub fn rfc3339(t: std::time::SystemTime) -> Option<String> {
    let ts = jiff::Timestamp::try_from(t).ok()?;
    let whole = jiff::Timestamp::from_second(ts.as_second()).ok()?;
    Some(whole.to_string())
}

fn history_path(env: &Env) -> PathBuf {
    env.state_home.join("crumbs/history.jsonl")
}

/// Appends `entries` as one write, then syncs. All or nothing: if any entry
/// can't be serialised, nothing is written. The executor calls this with
/// one entry per action so one bad entry never costs the others.
///
/// Known limitation: `PathBuf` fields holding non-UTF-8 paths cannot be
/// serialised by serde, so appending such an entry returns an error.
pub fn append(env: &Env, entries: &[HistoryEntry]) -> Result<(), CoreError> {
    let path = history_path(env);
    let dir = env.state_home.join("crumbs");
    DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&dir)
        .map_err(|e| io_err(&dir, e))?;
    // Opened without following a symlink, then everything goes through this
    // descriptor: a swapped path cannot redirect the chmod or the log.
    let dirfd = open_dir_nofollow(&dir).map_err(|e| io_err(&dir, e))?;
    // Warnings go out through `harden_files`, which the CLI calls at start-up.
    let _ = tighten_fds(&dirfd, &dir);
    let mut buf = Vec::new();
    for e in entries {
        serde_json::to_writer(&mut buf, e).map_err(|e| CoreError::Parse {
            what: "history entry".into(),
            message: e.to_string(),
        })?;
        buf.push(b'\n');
    }
    // Never follows a symlink: a planted link must not redirect the log.
    let fd = rustix::fs::openat(
        &dirfd,
        HISTORY_FILE,
        OFlags::RDWR | OFlags::APPEND | OFlags::CREATE | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        RawMode::from_raw_mode(0o600),
    )
    .map_err(|e| io_err(&path, e.into()))?;
    let f = fs::File::from(fd);
    // A torn last line (no newline) must not swallow the next entry.
    let len = f.metadata().map_err(|e| io_err(&path, e))?.len();
    if len > 0 {
        let mut last = [0u8; 1];
        f.read_exact_at(&mut last, len - 1)
            .map_err(|e| io_err(&path, e))?;
        if last[0] != b'\n' {
            buf.insert(0, b'\n');
        }
    }
    (&f).write_all(&buf).map_err(|e| io_err(&path, e))?;
    f.sync_data().map_err(|e| io_err(&path, e))
}

const HISTORY_FILE: &str = "history.jsonl";

/// Opens a directory without following a symlink at its last component.
fn open_dir_nofollow(dir: &Path) -> io::Result<OwnedFd> {
    rustix::fs::open(
        dir,
        OFlags::PATH | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        RawMode::empty(),
    )
    .map_err(io::Error::from)
}

/// `mode` with group and other bits removed and the owner's `owner` bits
/// forced on.
fn tightened(mode: u32, owner: u32) -> u32 {
    (mode & owner) | owner
}

/// chmod on the open `fd` (of `shown`) down to owner-only when it carries more.
fn tighten_one(fd: impl AsFd, shown: &Path, owner: u32, changed: &mut Vec<String>) {
    let Ok(st) = rustix::fs::fstat(&fd) else {
        return;
    };
    let mode = st.st_mode & 0o7777;
    let want = tightened(mode, owner);
    if want == mode {
        return;
    }
    // `fd` is an `O_PATH` descriptor (opened without needing any permission
    // bit, so a mode like 0o044 can be repaired), which `fchmod` rejects;
    // chmod through /proc/self/fd still acts on that exact inode.
    let via_proc = format!("/proc/self/fd/{}", fd.as_fd().as_raw_fd());
    match rustix::fs::chmod(&via_proc, RawMode::from_raw_mode(want)) {
        Ok(()) => changed.push(format!(
            "{} had mode {mode:o}; tightened to {want:o}",
            shown.display()
        )),
        Err(e) => changed.push(format!(
            "{} has mode {mode:o} and could not be tightened: {e}",
            shown.display()
        )),
    }
}

/// Tightens the history directory (open as `dirfd`) and the file in it to
/// owner-only, keeping the owner's bits (`0o700` / `0o600`). Returns what
/// was changed, one message each. A symlinked file is left alone.
fn tighten_fds(dirfd: &OwnedFd, dir: &Path) -> Vec<String> {
    let mut changed = Vec::new();
    tighten_one(dirfd, dir, 0o700, &mut changed);
    let file = dir.join(HISTORY_FILE);
    if let Ok(fd) = rustix::fs::openat(
        dirfd,
        HISTORY_FILE,
        OFlags::PATH | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        RawMode::empty(),
    ) && rustix::fs::fstat(&fd)
        .is_ok_and(|st| FileType::from_raw_mode(st.st_mode) == FileType::RegularFile)
    {
        tighten_one(&fd, &file, 0o600, &mut changed);
    }
    changed
}

/// Tightens loose modes on the history directory and file and returns a
/// message for each change, to be shown as `warning: history: <message>`.
/// Does nothing under a symlinked directory.
pub fn harden_files(env: &Env) -> Vec<String> {
    let dir = env.state_home.join("crumbs");
    match open_dir_nofollow(&dir) {
        Ok(fd) => tighten_fds(&fd, &dir),
        Err(_) => Vec::new(),
    }
}

/// Entries in file order plus the number of unparseable (non-blank) lines.
pub fn read_history(env: &Env) -> Result<(Vec<HistoryEntry>, usize), CoreError> {
    let path = history_path(env);
    // A symlinked history file is an error, as for `append`.
    let bytes = match OpenOptions::new()
        .read(true)
        .custom_flags(OFlags::NOFOLLOW.bits() as i32)
        .open(&path)
        .and_then(|mut f| {
            let mut b = Vec::new();
            io::Read::read_to_end(&mut f, &mut b).map(|_| b)
        }) {
        Ok(b) => b,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok((vec![], 0)),
        Err(e) => return Err(io_err(&path, e)),
    };
    let mut entries = Vec::new();
    let mut skipped = 0;
    for line in bytes.split(|&b| b == b'\n') {
        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        match serde_json::from_slice(line) {
            Ok(e) => entries.push(e),
            Err(_) => skipped += 1,
        }
    }
    Ok((entries, skipped))
}

#[cfg(test)]
mod tests;
