use std::fs::{self, DirBuilder, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::env::Env;
use crate::error::CoreError;
use crate::trash::Trashed;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HistoryAction {
    Trash,
    Delete,
    EditFile,
    Restore,
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
}

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

fn history_path(env: &Env) -> PathBuf {
    env.state_home.join("crumbs/history.jsonl")
}

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
    let mut buf = Vec::new();
    for e in entries {
        serde_json::to_writer(&mut buf, e).map_err(|e| CoreError::Parse {
            what: "history entry".into(),
            message: e.to_string(),
        })?;
        buf.push(b'\n');
    }
    let mut f = OpenOptions::new()
        .append(true)
        .create(true)
        .mode(0o600)
        .open(&path)
        .map_err(|e| io_err(&path, e))?;
    f.write_all(&buf).map_err(|e| io_err(&path, e))?;
    f.sync_data().map_err(|e| io_err(&path, e))
}

/// Entries in file order plus the number of unparseable (non-blank) lines.
pub fn read_history(env: &Env) -> Result<(Vec<HistoryEntry>, usize), CoreError> {
    let path = history_path(env);
    let bytes = match fs::read(&path) {
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
mod tests {
    use super::*;
    use crate::testing::FixtureTree;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    fn entry(i: usize, batch: &str) -> HistoryEntry {
        HistoryEntry {
            id: format!("{batch}-{i}"),
            batch: batch.into(),
            time: "2026-11-08T10:00:00Z".into(),
            action: HistoryAction::Trash,
            path: PathBuf::from(format!("/home/user/f{i}")),
            trashed: Some(Trashed {
                original: PathBuf::from(format!("/home/user/f{i}")),
                files_path: PathBuf::from("/t/files/f"),
                info_path: PathBuf::from("/t/info/f.trashinfo"),
            }),
            backup: None,
            size: 42,
            ok: true,
            error: None,
            app_id: Some("appimage:~/X.AppImage".into()),
            restores: None,
        }
    }

    #[test]
    fn history_append_and_read() {
        let t = FixtureTree::new();
        let env = t.env();
        assert_eq!(read_history(&env).unwrap(), (vec![], 0));
        let b = new_batch_id();
        assert!(b.parse::<u128>().is_ok());
        append(&env, &[entry(0, &b), entry(1, &b)]).unwrap();
        append(&env, &[entry(2, &b)]).unwrap();
        let (got, skipped) = read_history(&env).unwrap();
        assert_eq!(skipped, 0);
        assert_eq!(got, vec![entry(0, &b), entry(1, &b), entry(2, &b)]);
        let dir = env.state_home.join("crumbs");
        let mode = |p: &std::path::Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&dir), 0o700);
        assert_eq!(mode(&dir.join("history.jsonl")), 0o600);
        let text = fs::read_to_string(dir.join("history.jsonl")).unwrap();
        assert_eq!(text.lines().count(), 3);
        assert!(text.contains("\"action\":\"trash\""));
    }

    #[test]
    fn history_line_without_restores_parses() {
        let line = br#"{"id":"1-0","batch":"1","time":"t","action":"trash","path":"/home/user/f","trashed":null,"backup":null,"size":1,"ok":true,"error":null,"app_id":null}"#;
        let e: HistoryEntry = serde_json::from_slice(line).unwrap();
        assert_eq!(e.restores, None);
    }

    #[test]
    fn next_batch_id_is_after_existing() {
        let t = FixtureTree::new();
        let env = t.env();
        let future = "99999999999999".to_string();
        append(&env, &[entry(0, &future)]).unwrap();
        assert_eq!(next_batch_id(&env), "100000000000000");
    }

    #[test]
    fn history_skips_corrupt_line() {
        let t = FixtureTree::new();
        let env = t.env();
        append(&env, &[entry(0, "1")]).unwrap();
        let p = env.state_home.join("crumbs/history.jsonl");
        let mut text = fs::read_to_string(&p).unwrap();
        text.push_str("{not json\n\n");
        fs::write(&p, text).unwrap();
        append(&env, &[entry(1, "1")]).unwrap();
        let (got, skipped) = read_history(&env).unwrap();
        assert_eq!(got.len(), 2);
        assert_eq!(skipped, 1);
    }
}
