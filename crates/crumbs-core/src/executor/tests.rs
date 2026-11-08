use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use super::*;
use crate::command::CmdOutput;
use crate::history::{HistoryAction, read_history};
use crate::model::SourceKind;
use crate::plan::PlannedAction;
use crate::safety::{Fingerprint, Scope, fingerprint};
use crate::size::disk_usage;
use crate::testing::FakeRunner;
use crate::testing::FixtureTree;

/// Like `build_plan`: edits free nothing, removals free the disk usage.
fn planned(env: &Env, action: Action) -> PlannedAction {
    let p = action.path().unwrap().to_path_buf();
    let size = match action {
        Action::EditFile { .. } => 0,
        _ => disk_usage(env, &p).bytes,
    };
    PlannedAction {
        size,
        fingerprint: fingerprint(env, &p).unwrap(),
        scope: Scope::Home,
        depends_on: None,
        reason: "test".into(),
        action,
    }
}

fn plan_of(actions: Vec<PlannedAction>) -> Plan {
    Plan {
        app_id: Some("pacman:foo".into()),
        kind: Some("uninstall".into()),
        total_size: actions.iter().map(|a| a.size).sum(),
        actions,
        notes: Vec::new(),
    }
}

fn trash(p: &str) -> Action {
    Action::Trash { path: p.into() }
}

fn delete(p: &str) -> Action {
    Action::Delete { path: p.into() }
}

#[test]
fn execute_trashes_and_logs() {
    let t = FixtureTree::new();
    t.write("home/user/.config/foo/a", b"aaaa");
    t.write("home/user/.cache/foo/b", b"bbbb");
    let env = t.env();
    let plan = plan_of(vec![
        planned(&env, trash("/home/user/.config/foo")),
        planned(&env, trash("/home/user/.cache/foo")),
    ]);
    let report = execute(&env, &Config::default(), &plan);
    assert!(!report.batch.is_empty());
    assert_eq!(report.results.len(), 2);
    for (i, r) in report.results.iter().enumerate() {
        assert!(r.ok, "{r:?}");
        assert_eq!(r.error, None);
        assert_eq!(r.history_id, format!("{}-{i}", report.batch));
        assert_eq!(r.action, plan.actions[i].action);
    }
    assert_eq!(report.freed, plan.total_size);
    assert!(!env.sys("/home/user/.config/foo").exists());
    assert!(!env.sys("/home/user/.cache/foo").exists());

    let (hist, skipped) = read_history(&env).unwrap();
    assert_eq!(skipped, 0);
    assert_eq!(hist.len(), 2);
    for (i, h) in hist.iter().enumerate() {
        assert_eq!(h.id, report.results[i].history_id);
        assert_eq!(h.batch, report.batch);
        assert_eq!(h.action, HistoryAction::Trash);
        assert_eq!(Some(h.path.as_path()), plan.actions[i].action.path());
        assert!(h.ok);
        assert_eq!(h.app_id.as_deref(), Some("pacman:foo"));
        assert_eq!(h.size, plan.actions[i].size);
        let tr = h.trashed.as_ref().unwrap();
        assert_eq!(&tr.original, &h.path);
        assert!(env.sys_path(&tr.files_path).exists());
    }
}

/// Review Focus #1 end to end, for both trash and permanent delete.
#[test]
fn execute_refuses_swapped_path() {
    for permanent in [false, true] {
        let t = FixtureTree::new();
        t.write("home/user/.config/foo/a", b"a");
        t.write("home/user/Documents/precious", b"keep me");
        let env = t.env();
        let target = "/home/user/.config/foo";
        let action = if permanent {
            delete(target)
        } else {
            trash(target)
        };
        let plan = plan_of(vec![planned(&env, action)]);
        fs::remove_dir_all(env.sys(target)).unwrap();
        let docs = t.dir.path().join("home/user/Documents");
        std::os::unix::fs::symlink(&docs, env.sys(target)).unwrap();

        let report = execute(&env, &Config::default(), &plan);
        let r = &report.results[0];
        assert!(!r.ok);
        assert!(r.error.as_deref().unwrap().contains("changed"), "{r:?}");
        assert_eq!(report.freed, 0);
        assert_eq!(fs::read(docs.join("precious")).unwrap(), b"keep me");
        assert!(
            fs::symlink_metadata(env.sys(target))
                .unwrap()
                .file_type()
                .is_symlink()
        );
        let (hist, _) = read_history(&env).unwrap();
        assert_eq!(hist.len(), 1);
        assert!(!hist[0].ok);
        assert!(hist[0].error.is_some());
        assert_eq!(hist[0].trashed, None);
    }
}

#[test]
fn execute_partial_failure_continues() {
    let t = FixtureTree::new();
    for d in ["a", "b", "c"] {
        t.write(&format!("home/user/.config/{d}/f"), b"data");
    }
    let env = t.env();
    let plan = plan_of(vec![
        planned(&env, trash("/home/user/.config/a")),
        planned(&env, trash("/home/user/.config/b")),
        planned(&env, delete("/home/user/.config/c")),
    ]);
    fs::remove_dir_all(env.sys("/home/user/.config/b")).unwrap();
    let report = execute(&env, &Config::default(), &plan);
    let oks: Vec<bool> = report.results.iter().map(|r| r.ok).collect();
    assert_eq!(oks, [true, false, true]);
    assert!(
        report.results[1]
            .error
            .as_deref()
            .unwrap()
            .contains("no longer exists")
    );
    assert_eq!(report.freed, plan.actions[0].size + plan.actions[2].size);
    assert!(!env.sys("/home/user/.config/c").exists());
    let (hist, _) = read_history(&env).unwrap();
    let logged: Vec<(HistoryAction, bool)> = hist.iter().map(|h| (h.action, h.ok)).collect();
    assert_eq!(
        logged,
        [
            (HistoryAction::Trash, true),
            (HistoryAction::Trash, false),
            (HistoryAction::Delete, true)
        ]
    );
}

#[test]
fn execute_delete_removes_link_not_target() {
    let t = FixtureTree::new();
    t.write("home/user/target/keep", b"x");
    t.write("home/user/filelink-target", b"y");
    let env = t.env();
    t.symlink("home/user/.config/dirlink", "../target");
    t.symlink("home/user/.config/filelink", "../filelink-target");
    let plan = plan_of(vec![
        planned(&env, delete("/home/user/.config/dirlink")),
        planned(&env, delete("/home/user/.config/filelink")),
    ]);
    let report = execute(&env, &Config::default(), &plan);
    assert!(report.results.iter().all(|r| r.ok), "{report:?}");
    assert!(fs::symlink_metadata(env.sys("/home/user/.config/dirlink")).is_err());
    assert!(fs::symlink_metadata(env.sys("/home/user/.config/filelink")).is_err());
    assert!(env.sys("/home/user/target/keep").exists());
    assert!(env.sys("/home/user/filelink-target").exists());
}

#[test]
fn execute_refuses_protected_target() {
    let t = FixtureTree::new();
    t.write("home/user/.ssh/id", b"k");
    let env = t.env();
    let plan = plan_of(vec![planned(&env, delete("/home/user/.ssh/id"))]);
    let report = execute(&env, &Config::default(), &plan);
    assert!(!report.results[0].ok);
    assert!(env.sys("/home/user/.ssh/id").exists());
}

const MIME: &[u8] = b"[Default Applications]\r\ntext/plain=foo.desktop\r\nimage/png=other.desktop\n\n[Added Associations]\nx-scheme-handler/f=foo.desktop;\ntext/plain=foo.desktop;bar.desktop\n";

fn mime_edit() -> Action {
    Action::EditFile {
        path: "/home/user/.config/mimeapps.list".into(),
        remove_lines: vec![
            "text/plain=foo.desktop".into(),
            "x-scheme-handler/f=foo.desktop;".into(),
            String::new(),
        ],
    }
}

#[test]
fn edit_file_backup_and_filter() {
    let t = FixtureTree::new();
    let real = t.write("home/user/.config/mimeapps.list", MIME);
    fs::set_permissions(&real, fs::Permissions::from_mode(0o640)).unwrap();
    let env = t.env();
    let plan = plan_of(vec![planned(&env, mime_edit())]);
    let report = execute(&env, &Config::default(), &plan);
    assert!(report.results[0].ok, "{report:?}");
    assert_eq!(report.freed, 0);
    assert_eq!(
        fs::read(&real).unwrap(),
        b"[Default Applications]\r\nimage/png=other.desktop\n\n[Added Associations]\ntext/plain=foo.desktop;bar.desktop\n"
    );
    assert_eq!(
        fs::metadata(&real).unwrap().permissions().mode() & 0o777,
        0o640
    );
    let backup = PathBuf::from(format!(
        "/home/user/.config/mimeapps.list.crumbs-bak-{}",
        report.batch
    ));
    assert_eq!(fs::read(env.sys_path(&backup)).unwrap(), MIME);
    let (hist, _) = read_history(&env).unwrap();
    assert_eq!(hist[0].action, HistoryAction::EditFile);
    assert_eq!(hist[0].backup.as_deref(), Some(backup.as_path()));
    // No temp file left behind.
    let names: Vec<String> = fs::read_dir(env.sys("/home/user/.config"))
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names.len(), 2, "{names:?}");

    // Nothing left to remove: ok, and no second backup.
    let plan = plan_of(vec![planned(&env, mime_edit())]);
    let again = execute(&env, &Config::default(), &plan);
    assert!(again.results[0].ok);
    assert!(
        !env.sys_path(Path::new(&format!(
            "/home/user/.config/mimeapps.list.crumbs-bak-{}",
            again.batch
        )))
        .exists()
    );
    let (hist, _) = read_history(&env).unwrap();
    assert_eq!(hist[1].backup, None);
}

#[test]
fn edit_file_refuses_symlink() {
    let t = FixtureTree::new();
    t.write("home/user/elsewhere.list", MIME);
    t.symlink("home/user/.config/mimeapps.list", "../elsewhere.list");
    let env = t.env();
    let plan = plan_of(vec![planned(&env, mime_edit())]);
    let report = execute(&env, &Config::default(), &plan);
    assert!(!report.results[0].ok);
    assert_eq!(
        report.results[0].error.as_deref(),
        Some("refusing: ~/.config/mimeapps.list is a symlink; edit it by hand")
    );
    assert!(
        fs::symlink_metadata(env.sys("/home/user/.config/mimeapps.list"))
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(
        fs::read(env.sys("/home/user/elsewhere.list")).unwrap(),
        MIME
    );
}

#[test]
fn delete_reports_partial_removal() {
    if rustix::process::getuid().is_root() {
        return; // root ignores directory permissions
    }
    let t = FixtureTree::new();
    t.write("home/user/.config/foo/a.txt", b"a");
    t.write("home/user/.config/foo/locked/inner", b"i");
    let locked = t.dir.path().join("home/user/.config/foo/locked");
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o500)).unwrap();
    let env = t.env();
    let plan = plan_of(vec![planned(&env, delete("/home/user/.config/foo"))]);
    let report = execute(&env, &Config::default(), &plan);
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o700)).unwrap();
    let r = &report.results[0];
    assert!(!r.ok);
    let err = r.error.as_deref().unwrap();
    assert!(err.starts_with("partially deleted: "), "{err}");
    assert!(env.sys("/home/user/.config/foo/locked/inner").exists());
}

#[test]
fn history_failure_is_reported() {
    let t = FixtureTree::new();
    t.write("home/user/.config/foo/a", b"a");
    // A file where the state dir should be makes the append fail.
    t.write("home/user/.local/state/crumbs", b"not a dir");
    let env = t.env();
    let plan = plan_of(vec![planned(&env, trash("/home/user/.config/foo"))]);
    let report = execute(&env, &Config::default(), &plan);
    let err = report.results[0].error.as_deref().unwrap();
    assert!(err.ends_with("; history not written"), "{err}");
}

fn appimage_app() -> App {
    let mut app = App::new(SourceKind::AppImage, "Foo.AppImage");
    app.location = Some("/home/user/Apps/Foo.AppImage".into());
    app.exec_names = vec!["foo-bin".into()];
    app
}

#[test]
fn running_process_detected_by_exe() {
    let t = FixtureTree::new();
    let env = t.env();
    t.symlink("proc/1234/exe", "/usr/bin/foo-bin");
    t.symlink("proc/99/exe", "/usr/bin/other");
    t.symlink("proc/self/exe", "/usr/bin/foo-bin");
    let loc = t.dir.path().join("home/user/Apps/Foo.AppImage");
    t.symlink("proc/555/exe", loc.to_str().unwrap());
    t.write("proc/777/status", b"no exe here");
    assert_eq!(running_processes(&env, &appimage_app()), [555, 1234]);
}

#[test]
fn running_process_detected_by_appimage_env() {
    let t = FixtureTree::new();
    let env = t.env();
    t.write(
        "proc/42/environ",
        b"HOME=/home/user\0APPIMAGE=/home/user/Apps/Foo.AppImage\0X=1\0",
    );
    t.write(
        "proc/43/environ",
        b"APPIMAGE=/home/user/Apps/Foo.AppImage.old\0",
    );
    t.symlink("proc/43/exe", "/tmp/.mount_x/usr/bin/other");
    assert_eq!(running_processes(&env, &appimage_app()), [42]);
}

#[test]
fn running_processes_without_proc_is_empty() {
    let t = FixtureTree::new();
    let env = t.env();
    assert!(running_processes(&env, &appimage_app()).is_empty());
}

/// A plan's scope reaches `verify`: a `/tmp` child is deleted under
/// `UserTemp` and refused under the default `Home`.
#[test]
fn execute_passes_scope_to_verify() {
    let t = FixtureTree::new();
    let a = t.write("tmp/a", b"a");
    age_days(&a, 30);
    t.write("tmp/b", b"b");
    let env = temp_env(&t);
    let mut temp = planned(&env, delete("/tmp/a"));
    temp.scope = Scope::UserTemp;
    let home = planned(&env, delete("/tmp/b"));
    assert_eq!(home.scope, Scope::Home);
    let report = execute(&env, &Config::default(), &plan_of(vec![temp, home]));
    assert!(report.results[0].ok, "{:?}", report.results[0]);
    assert!(!report.results[1].ok);
    assert_eq!(
        report.results[1].error.as_deref(),
        Some("refusing: /tmp/b is outside the allowed roots")
    );
    assert!(!env.sys("/tmp/a").exists());
    assert!(env.sys("/tmp/b").exists());
}

/// `--permanent` never descends into another filesystem: a tree holding a
/// mount point is refused untouched. Mounts can't be made in tests, so
/// the device lookup is faked for one subdirectory.
#[test]
fn delete_refuses_tree_with_another_filesystem() {
    use std::os::unix::fs::MetadataExt;
    let t = FixtureTree::new();
    t.write("home/user/.config/foo/a", b"a");
    t.write("home/user/.config/foo/mnt/inner", b"on another fs");
    let env = t.env();
    let mnt = env.sys("/home/user/.config/foo/mnt");
    // A same-device bind mount: only the mount ID differs.
    let fake_mount = |p: &Path, _: &fs::Metadata| Some(if p.starts_with(&mnt) { 2 } else { 1 });
    let err = delete_with(&env, Path::new("/home/user/.config/foo"), &fake_mount).unwrap_err();
    assert_eq!(err, "refusing: ~/.config/foo contains another filesystem");
    assert!(env.sys("/home/user/.config/foo/a").exists());
    assert!(env.sys("/home/user/.config/foo/mnt/inner").exists());

    // Same device everywhere: deleted as before.
    let same = |_: &Path, m: &fs::Metadata| Some(m.dev());
    delete_with(&env, Path::new("/home/user/.config/foo"), &same).unwrap();
    assert!(!env.sys("/home/user/.config/foo").exists());
}

/// Each action's history entry is its own line: one entry that can't be
/// written (a non-UTF-8 path can't be serialised) loses only that entry.
#[test]
fn history_entry_failure_is_per_action() {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;
    let t = FixtureTree::new();
    t.write("home/user/.config/a/f", b"a");
    t.write("home/user/.config/c/f", b"c");
    let env = t.env();
    let bad = Path::new("/home/user/.config").join(OsStr::from_bytes(b"b\xff"));
    fs::write(env.sys_path(&bad), b"b").unwrap();
    let plan = plan_of(vec![
        planned(&env, trash("/home/user/.config/a")),
        planned(&env, Action::Trash { path: bad.clone() }),
        planned(&env, trash("/home/user/.config/c")),
    ]);
    let report = execute(&env, &Config::default(), &plan);
    assert!(report.results.iter().all(|r| r.ok), "{report:?}");
    assert_eq!(report.results[0].error, None);
    assert_eq!(report.results[2].error, None);
    let err = report.results[1].error.as_deref().unwrap();
    assert!(err.ends_with("; history not written"), "{err}");
    let (hist, skipped) = read_history(&env).unwrap();
    assert_eq!(skipped, 0);
    let ids: Vec<&str> = hist.iter().map(|h| h.id.as_str()).collect();
    assert_eq!(
        ids,
        [
            report.results[0].history_id.as_str(),
            report.results[2].history_id.as_str()
        ]
    );
}

/// A target that is itself a mount root (a FUSE mount in `/tmp`, say) is
/// refused untouched: its mount differs from its parent's.
#[test]
fn delete_refuses_target_that_is_a_mount_root() {
    let t = FixtureTree::new();
    t.write("tmp/mnt/inner", b"on another fs");
    let env = t.env();
    let mnt = env.sys("/tmp/mnt");
    let fake_mount = |p: &Path, _: &fs::Metadata| Some(if p.starts_with(&mnt) { 2 } else { 1 });
    let err = delete_with(&env, Path::new("/tmp/mnt"), &fake_mount).unwrap_err();
    assert_eq!(err, "refusing: /tmp/mnt contains another filesystem");
    assert!(env.sys("/tmp/mnt/inner").exists());
}

#[test]
fn leftovers_skipped_when_appimage_file_fails() {
    if rustix::process::getuid().is_root() {
        return; // root ignores directory permissions
    }
    let t = FixtureTree::new();
    t.appimage("home/user/Apps/Foo.AppImage");
    t.write("home/user/.config/foo/a", b"a");
    t.write("home/user/.cache/foo/b", b"b");
    // A read-only Trash/info makes trashing anything fail.
    t.mkdir("home/user/.local/share/Trash/files");
    let info = t.mkdir("home/user/.local/share/Trash/info");
    let env = t.env();
    let mut actions = vec![
        planned(&env, trash("/home/user/Apps/Foo.AppImage")),
        planned(&env, trash("/home/user/.config/foo")),
        planned(&env, trash("/home/user/.cache/foo")),
    ];
    for a in &mut actions[1..] {
        a.depends_on = Some(0);
    }
    let plan = plan_of(actions);
    fs::set_permissions(&info, fs::Permissions::from_mode(0o500)).unwrap();
    let report = execute(&env, &Config::default(), &plan);
    fs::set_permissions(&info, fs::Permissions::from_mode(0o700)).unwrap();

    assert!(!report.results[0].ok);
    for r in &report.results[1..] {
        assert!(!r.ok);
        assert_eq!(
            r.error.as_deref(),
            Some("skipped: ~/Apps/Foo.AppImage was not removed")
        );
    }
    assert_eq!(report.freed, 0);
    assert!(env.sys("/home/user/.config/foo/a").exists());
    assert!(env.sys("/home/user/.cache/foo/b").exists());
    let (hist, _) = read_history(&env).unwrap();
    assert_eq!(hist.len(), 3);
    for (h, r) in hist.iter().zip(&report.results).skip(1) {
        assert_eq!(h.id, r.history_id);
        assert!(!h.ok);
        assert_eq!(h.action, HistoryAction::Trash);
        assert_eq!(h.error, r.error);
        assert!(h.trashed.is_none());
    }
}

#[test]
fn dependents_run_when_dependency_succeeds() {
    let t = FixtureTree::new();
    t.write("home/user/.config/foo/a", b"a");
    t.write("home/user/.cache/foo/b", b"b");
    let env = t.env();
    let mut second = planned(&env, trash("/home/user/.cache/foo"));
    second.depends_on = Some(0);
    let plan = plan_of(vec![planned(&env, trash("/home/user/.config/foo")), second]);
    let report = execute(&env, &Config::default(), &plan);
    assert!(report.results.iter().all(|r| r.ok), "{report:?}");
}

#[test]
fn invalid_dependencies_fail_closed_and_skips_are_transitive() {
    if rustix::process::getuid().is_root() {
        return; // root ignores directory permissions
    }
    let t = FixtureTree::new();
    t.write("home/user/.config/a/x", b"a");
    t.write("home/user/.config/b/x", b"b");
    t.write("home/user/.config/c/x", b"c");
    t.write("home/user/.config/d/x", b"d");
    t.write("home/user/.config/e/x", b"e");
    t.mkdir("home/user/.local/share/Trash/files");
    let info = t.mkdir("home/user/.local/share/Trash/info");
    let env = t.env();
    // 0 fails; 1 depends on 0; 2 on 1 (transitive); 3 on itself; 4 forward.
    let mut actions: Vec<PlannedAction> = ["a", "b", "c", "d", "e"]
        .iter()
        .map(|n| planned(&env, trash(&format!("/home/user/.config/{n}"))))
        .collect();
    actions[1].depends_on = Some(0);
    actions[2].depends_on = Some(1);
    actions[3].depends_on = Some(3);
    actions[4].depends_on = Some(9);
    let plan = plan_of(actions);
    fs::set_permissions(&info, fs::Permissions::from_mode(0o500)).unwrap();
    let report = execute(&env, &Config::default(), &plan);
    fs::set_permissions(&info, fs::Permissions::from_mode(0o700)).unwrap();
    let errs: Vec<&str> = report
        .results
        .iter()
        .map(|r| r.error.as_deref().unwrap())
        .collect();
    assert!(report.results.iter().all(|r| !r.ok));
    assert_eq!(errs[1], "skipped: ~/.config/a was not removed");
    assert_eq!(errs[2], "skipped: ~/.config/b was not removed");
    assert_eq!(errs[3], "skipped: ~/.config/d was not removed");
    assert_eq!(errs[4], "skipped: action 9 was not removed");
    for n in ["a", "b", "c", "d", "e"] {
        assert!(env.sys(&format!("/home/user/.config/{n}/x")).exists());
    }
}

/// Days the clock of [`temp_env`] runs ahead: an entry made now already
/// has an old ctime (which cannot be set), so its mtime decides its age.
const SHIFT_DAYS: u64 = 100;

/// The fixture's env with its clock [`SHIFT_DAYS`] ahead.
fn temp_env(t: &FixtureTree) -> crate::env::Env {
    let mut env = t.env();
    env.clock_shift = std::time::Duration::from_secs(SHIFT_DAYS * 86_400);
    env
}

/// Sets atime and mtime of `p` to `days` before the [`temp_env`] clock,
/// without following a symlink.
fn age_days(p: &Path, days: u64) {
    use rustix::fs::{AtFlags, CWD, Timespec, Timestamps, utimensat};
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
        + SHIFT_DAYS * 86_400
        - days * 86_400;
    let ts = Timespec {
        tv_sec: secs as i64,
        tv_nsec: 0,
    };
    let times = Timestamps {
        last_access: ts,
        last_modification: ts,
    };
    utimensat(CWD, p, &times, AtFlags::SYMLINK_NOFOLLOW).unwrap();
}

/// A `/tmp` dir that was stale when scanned but gained a fresh file
/// before execution is refused and left intact.
#[test]
fn execute_rechecks_temp_staleness() {
    let t = FixtureTree::new();
    let f = t.write("tmp/work/old", b"x");
    let dir = env_dir(&t, "tmp/work");
    age_days(&f, 30);
    age_days(&dir, 30);
    let g = t.write("tmp/gone/old", b"x");
    let gone = env_dir(&t, "tmp/gone");
    age_days(&g, 30);
    age_days(&gone, 30);
    let env = temp_env(&t);
    let mut stale = planned(&env, delete("/tmp/work"));
    stale.scope = Scope::UserTemp;
    let mut ok = planned(&env, delete("/tmp/gone"));
    ok.scope = Scope::UserTemp;
    // Someone starts using the dir between scan and clean. Adding an entry
    // bumps the dir's ctime, which `verify` now catches by itself, so the
    // fingerprint is retaken to isolate the staleness recheck.
    age_days(&t.write("tmp/work/new", b"fresh"), 0);
    age_days(&dir, 30);
    stale.fingerprint = fingerprint(&env, Path::new("/tmp/work")).unwrap();
    let report = execute(&env, &Config::default(), &plan_of(vec![stale, ok]));
    assert!(!report.results[0].ok);
    assert_eq!(
        report.results[0].error.as_deref(),
        Some("refusing: /tmp/work is no longer an old temporary file")
    );
    assert!(env.sys("/tmp/work/old").exists());
    assert!(env.sys("/tmp/work/new").exists());
    assert!(report.results[1].ok, "{:?}", report.results[1]);
    assert!(!env.sys("/tmp/gone").exists());
}

fn env_dir(t: &FixtureTree, rel: &str) -> PathBuf {
    t.dir.path().join(rel)
}

fn command_plan(program: &str, args: &[&str]) -> Plan {
    plan_of(vec![PlannedAction {
        action: Action::UserCommand {
            program: program.into(),
            args: args.iter().map(|a| (*a).to_string()).collect(),
        },
        size: 0,
        fingerprint: Fingerprint {
            dev: 0,
            ino: 0,
            is_symlink: false,
            ctime: 0,
            ctime_nsec: 0,
        },
        scope: Scope::Home,
        depends_on: None,
        reason: "test".into(),
    }])
}

#[test]
fn user_command_runs_trusted_program() {
    let t = FixtureTree::new();
    let log = t.dir.path().join("args.log");
    let script = format!("#!/bin/sh\necho \"$@\" > {}\n", log.display());
    t.write_exec("usr/bin/flatpak", script.as_bytes());
    let env = t.env();
    let plan = command_plan("flatpak", &["uninstall", "--user", "--unused"]);
    let report = execute(&env, &Config::default(), &plan);
    assert!(report.results[0].ok, "{:?}", report.results[0]);
    assert_eq!(
        fs::read_to_string(&log).unwrap(),
        "uninstall --user --unused\n"
    );
    assert_eq!(report.freed, 0);
    let (entries, _) = read_history(&env).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].action, HistoryAction::Command);
    assert_eq!(entries[0].path, Path::new("/usr/bin/flatpak"));
    assert_eq!(
        entries[0].argv.as_deref(),
        Some(&["uninstall".to_string(), "--user".into(), "--unused".into()][..])
    );
    assert!(entries[0].ok);
    assert_eq!(entries[0].size, 0);
    let line = fs::read_to_string(env.state_home.join("crumbs/history.jsonl")).unwrap();
    assert!(line.contains("\"action\":\"command\""), "{line}");
}

#[test]
fn user_command_missing_program() {
    let t = FixtureTree::new();
    // Only $HOME/.local/bin has it: not a trusted directory.
    t.write_exec("home/user/.local/bin/flatpak", b"#!/bin/sh\n");
    let env = t.env();
    let runner = FakeRunner::new();
    let plan = command_plan("flatpak", &["uninstall"]);
    let report = execute_with(&env, &Config::default(), &plan, &runner);
    assert!(!report.results[0].ok);
    assert_eq!(
        report.results[0].error.as_deref(),
        Some("flatpak is not installed")
    );
    assert!(runner.calls().is_empty());
    let (entries, _) = read_history(&env).unwrap();
    assert_eq!(entries[0].action, HistoryAction::Command);
    assert!(!entries[0].ok);
}

#[test]
fn user_command_failure_reports_last_stderr_line() {
    let t = FixtureTree::new();
    t.write_exec("usr/bin/flatpak", b"#!/bin/sh\n");
    let env = t.env();
    let out = |status, stderr: &str| CmdOutput {
        status,
        stdout: String::new(),
        stderr: stderr.into(),
        timed_out: false,
    };
    let runner = FakeRunner::new()
        .on("flatpak", &["a"], out(1, "first\nerror: boom\n\n"))
        .on("flatpak", &["b"], out(3, ""));
    let mut plan = command_plan("flatpak", &["a"]);
    plan.actions.extend(command_plan("flatpak", &["b"]).actions);
    let report = execute_with(&env, &Config::default(), &plan, &runner);
    assert_eq!(report.results[0].error.as_deref(), Some("error: boom"));
    assert_eq!(
        report.results[1].error.as_deref(),
        Some("exited with status 3")
    );
}

#[test]
fn user_command_timeout_is_a_failure() {
    let t = FixtureTree::new();
    t.write_exec("usr/bin/flatpak", b"#!/bin/sh\n");
    let env = t.env();
    let runner = FakeRunner::new().on(
        "flatpak",
        &["a"],
        CmdOutput {
            status: -1,
            stdout: String::new(),
            stderr: String::new(),
            timed_out: true,
        },
    );
    let report = execute_with(
        &env,
        &Config::default(),
        &command_plan("flatpak", &["a"]),
        &runner,
    );
    assert!(!report.results[0].ok);
    assert_eq!(report.results[0].error.as_deref(), Some("timed out"));
}

#[test]
fn temp_delete_verified_in_user_temp_scope() {
    use crate::bloat::{BloatItem, Category, ItemAction};
    let t = FixtureTree::new();
    let a = t.write("tmp/a", b"a");
    let b = t.write("tmp/b", b"b");
    age_days(&a, 30);
    age_days(&b, 30);
    let env = temp_env(&t);
    let item = |path: &str| {
        let p = PathBuf::from(path);
        BloatItem {
            category: Category::Temp,
            size: Some(1),
            fingerprint: Some(fingerprint(&env, &p).unwrap()),
            path: Some(p),
            label: path.into(),
            rebuildable: false,
            preselected: true,
            note: None,
            action: ItemAction::Delete,
            scope: Scope::UserTemp,
            extra_paths: Vec::new(),
            app_id: None,
        }
    };
    let items = [item("/tmp/a"), item("/tmp/b")];
    let plan = crate::plan::build_clean_plan(&env, &items, &Default::default()).unwrap();
    // /tmp/b is swapped for a different inode after the scan.
    fs::remove_file(env.sys("/tmp/b")).unwrap();
    let nb = t.write("tmp/b", b"other");
    age_days(&nb, 30);
    let report = execute(&env, &Config::default(), &plan);
    assert!(report.results[0].ok, "{:?}", report.results[0]);
    assert!(!env.sys("/tmp/a").exists());
    assert!(!report.results[1].ok);
    assert!(env.sys("/tmp/b").exists());
    let (entries, _) = read_history(&env).unwrap();
    assert_eq!(entries[0].action, HistoryAction::Delete);
    assert_eq!(entries[0].path, Path::new("/tmp/a"));
}
