#[test]
fn rfc3339_is_whole_seconds_utc_and_refuses_out_of_range() {
    use std::time::{Duration, UNIX_EPOCH};
    let at = |secs: u64| super::rfc3339(UNIX_EPOCH + Duration::from_secs(secs));
    assert_eq!(at(0).as_deref(), Some("1970-01-01T00:00:00Z"));
    assert_eq!(at(951_868_800).as_deref(), Some("2000-03-01T00:00:00Z"));
    assert_eq!(at(4_102_444_800).as_deref(), Some("2100-01-01T00:00:00Z"));
    // A fraction is dropped, not rounded.
    let fraction = UNIX_EPOCH + Duration::from_millis(951_868_800_999);
    assert_eq!(
        super::rfc3339(fraction).as_deref(),
        Some("2000-03-01T00:00:00Z")
    );
    // Past year 9999: no timestamp, not the epoch.
    assert_eq!(at(300_000_000_000), None);
}

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
        kind: Some(KIND_UNINSTALL.into()),
        restores: None,
        argv: None,
        packages: None,
        source: None,
        scope: None,
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
fn history_refuses_symlink() {
    let t = FixtureTree::new();
    let env = t.env();
    let dir = env.state_home.join("crumbs");
    fs::create_dir_all(&dir).unwrap();
    let target = t.write("home/user/elsewhere", b"");
    std::os::unix::fs::symlink(&target, dir.join("history.jsonl")).unwrap();
    assert!(append(&env, &[entry(0, "1")]).is_err());
    assert_eq!(fs::read(&target).unwrap(), b"");
}

#[test]
fn history_tightens_modes() {
    let t = FixtureTree::new();
    let env = t.env();
    append(&env, &[entry(0, "1")]).unwrap();
    let dir = env.state_home.join("crumbs");
    let file = dir.join("history.jsonl");
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();
    fs::set_permissions(&file, fs::Permissions::from_mode(0o644)).unwrap();
    let warnings = harden_files(&env);
    assert_eq!(warnings.len(), 2, "{warnings:?}");
    let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(&dir), 0o700);
    assert_eq!(mode(&file), 0o600);
    assert!(harden_files(&env).is_empty());
    // `append` alone tightens too.
    fs::set_permissions(&file, fs::Permissions::from_mode(0o666)).unwrap();
    append(&env, &[entry(1, "1")]).unwrap();
    assert_eq!(mode(&file), 0o600);
}

#[test]
fn history_refuses_symlinked_dir() {
    let t = FixtureTree::new();
    let env = t.env();
    let elsewhere = t.dir.path().join("elsewhere");
    fs::create_dir_all(&elsewhere).unwrap();
    fs::create_dir_all(&env.state_home).unwrap();
    std::os::unix::fs::symlink(&elsewhere, env.state_home.join("crumbs")).unwrap();
    assert!(append(&env, &[entry(0, "1")]).is_err());
    assert!(!elsewhere.join("history.jsonl").exists());
}

#[test]
fn harden_skips_symlinked_dir() {
    let t = FixtureTree::new();
    let env = t.env();
    let elsewhere = t.dir.path().join("elsewhere");
    fs::create_dir_all(&elsewhere).unwrap();
    fs::set_permissions(&elsewhere, fs::Permissions::from_mode(0o755)).unwrap();
    let f = elsewhere.join("history.jsonl");
    fs::write(&f, b"").unwrap();
    fs::set_permissions(&f, fs::Permissions::from_mode(0o644)).unwrap();
    fs::create_dir_all(&env.state_home).unwrap();
    std::os::unix::fs::symlink(&elsewhere, env.state_home.join("crumbs")).unwrap();
    assert!(harden_files(&env).is_empty());
    let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(&f), 0o644);
    assert_eq!(mode(&elsewhere), 0o755);
}

#[test]
fn tightening_keeps_owner_bits() {
    let t = FixtureTree::new();
    let env = t.env();
    append(&env, &[entry(0, "1")]).unwrap();
    let dir = env.state_home.join("crumbs");
    let file = dir.join("history.jsonl");
    fs::set_permissions(&file, fs::Permissions::from_mode(0o044)).unwrap();
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o055)).unwrap();
    assert_eq!(harden_files(&env).len(), 2);
    let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(&file), 0o600);
    assert_eq!(mode(&dir), 0o700);
}

#[test]
fn history_read_refuses_symlink() {
    let t = FixtureTree::new();
    let env = t.env();
    let dir = env.state_home.join("crumbs");
    fs::create_dir_all(&dir).unwrap();
    let target = t.write("home/user/elsewhere", b"");
    std::os::unix::fs::symlink(&target, dir.join("history.jsonl")).unwrap();
    assert!(read_history(&env).is_err());
}

#[test]
fn history_repairs_torn_line() {
    let t = FixtureTree::new();
    let env = t.env();
    append(&env, &[entry(0, "1")]).unwrap();
    let p = env.state_home.join("crumbs/history.jsonl");
    let mut text = fs::read_to_string(&p).unwrap();
    text.push_str("{\"id\":\"torn");
    fs::write(&p, text).unwrap();
    append(&env, &[entry(1, "1")]).unwrap();
    let (got, skipped) = read_history(&env).unwrap();
    assert_eq!(got, vec![entry(0, "1"), entry(1, "1")]);
    assert_eq!(skipped, 1);
}

#[test]
fn history_line_without_restores_parses() {
    let line = br#"{"id":"1-0","batch":"1","time":"t","action":"trash","path":"/home/user/f","trashed":null,"backup":null,"size":1,"ok":true,"error":null,"app_id":null}"#;
    let e: HistoryEntry = serde_json::from_slice(line).unwrap();
    assert_eq!(e.restores, None);
}

#[test]
fn edit_file_action_is_kebab_case_and_reads_old_spelling() {
    let v = serde_json::to_value(HistoryAction::EditFile).unwrap();
    assert_eq!(v, "edit-file");
    for spelling in ["\"edit-file\"", "\"edit_file\""] {
        let a: HistoryAction = serde_json::from_str(spelling).unwrap();
        assert_eq!(a, HistoryAction::EditFile, "{spelling}");
    }
    let line = br#"{"id":"1-0","batch":"1","time":"t","action":"edit_file","path":"/home/user/.config/mimeapps.list","trashed":null,"backup":"/home/user/.config/mimeapps.list.crumbs-bak-1","size":0,"ok":true,"error":null,"app_id":null,"restores":null}"#;
    let e: HistoryEntry = serde_json::from_slice(line).unwrap();
    assert_eq!(e.action, HistoryAction::EditFile);
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
