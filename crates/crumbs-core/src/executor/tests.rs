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
        depends_on_plan: None,
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
fn running_detects_deleted_exe_and_comm() {
    let t = FixtureTree::new();
    let env = t.env();
    // The binary was replaced by an update: the kernel appends " (deleted)".
    t.symlink("proc/10/exe", "/usr/bin/foo-bin (deleted)");
    // Only the process name and argv0 say who it is.
    t.write("proc/11/comm", b"foo-bin\n");
    t.write("proc/12/cmdline", b"/opt/foo/foo-bin\0--flag\0");
    // The kernel cuts comm at 15 bytes.
    t.write("proc/13/comm", b"very-long-name-\n");
    t.write("proc/14/comm", b"unrelated\n");
    t.write("proc/14/cmdline", b"/usr/bin/unrelated\0");
    let mut app = appimage_app();
    app.exec_names.push("very-long-name-app".into());
    assert_eq!(running_processes(&env, &app), [10, 11, 12, 13]);
}

#[test]
fn generic_basenames_never_match() {
    let t = FixtureTree::new();
    let env = t.env();
    let mut app = appimage_app();
    app.exec_names = ["sh", "bash", "env", "python", "python3", "node"]
        .into_iter()
        .chain(["electron", "java", "flatpak", "bwrap", "/usr/bin/bash"])
        .chain(["dash", "zsh", "fish", "perl", "ruby", "python3.12"])
        .map(String::from)
        .collect();
    for (pid, name) in [(1, "sh"), (2, "bash"), (3, "python3"), (4, "flatpak")] {
        t.symlink(&format!("proc/{pid}/exe"), &format!("/usr/bin/{name}"));
    }
    t.write("proc/5/comm", b"node\n");
    t.write("proc/6/cmdline", b"/usr/bin/java\0-jar\0x.jar\0");
    t.write("proc/7/cmdline", b"bwrap\0--x\0");
    for (pid, name) in [
        (20, "dash"),
        (21, "zsh"),
        (22, "fish"),
        (23, "perl"),
        (24, "ruby"),
        (25, "python3.12"),
    ] {
        t.symlink(&format!("proc/{pid}/exe"), &format!("/usr/bin/{name}"));
    }
    assert!(running_processes(&env, &app).is_empty());
    // The exact location still matches.
    let loc = t.dir.path().join("home/user/Apps/Foo.AppImage");
    t.symlink("proc/8/exe", loc.to_str().unwrap());
    assert_eq!(running_processes(&env, &app), [8]);
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
    // Someone starts using the dir between scan and clean; the dir's own
    // inode is unchanged, so `verify` alone would pass.
    age_days(&t.write("tmp/work/new", b"fresh"), 0);
    age_days(&dir, 30);
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
        depends_on_plan: None,
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
    assert_eq!(
        report.results[0].error.as_deref(),
        Some("flatpak timed out after 30 min")
    );
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

// ---- Privileged batch (M4 Task 4) ----

use crate::privilege::batch::PrivilegedRunner;
use crate::privilege::{Escalation, EscalationKind};
use crate::testing::FakePrivilegedRunner;

fn esc() -> Escalation {
    Escalation {
        kind: EscalationKind::Pkexec,
        program: PathBuf::from("/usr/bin/pkexec"),
        requires_tty: false,
    }
}

fn no_post_check(_: &Env, _: SourceKind, _: &[String]) -> Result<Vec<String>, String> {
    Ok(Vec::new())
}

/// Steps in a script: one `__c <i>` or `__t<n> <i>` line per step.
fn step_count(script: &[u8]) -> usize {
    String::from_utf8_lossy(script)
        .lines()
        .filter(|l| {
            let word = l.split(' ').next().unwrap_or_default();
            word == "__c"
                || word
                    .strip_prefix("__t")
                    .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
        })
        .count()
}

fn unfingerprinted(action: Action) -> PlannedAction {
    PlannedAction {
        action,
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
        depends_on_plan: None,
        reason: "test".into(),
    }
}

fn priv_cmd(program: &str, args: &[&str]) -> PlannedAction {
    unfingerprinted(Action::PrivilegedCommand {
        program: program.into(),
        args: args.iter().map(|a| (*a).to_string()).collect(),
    })
}

fn pkg_remove(names: &[&str], versions: &[&str]) -> PlannedAction {
    let mut args = vec!["-Rns".to_string(), "--noconfirm".into()];
    args.extend(names.iter().map(|n| (*n).to_string()));
    let mut pa = unfingerprinted(Action::PackageRemove {
        source: SourceKind::Pacman,
        names: names.iter().map(|n| (*n).to_string()).collect(),
        versions: versions.iter().map(|v| (*v).to_string()).collect(),
        program: "pacman".into(),
        args,
    });
    pa.size = 100;
    pa
}

/// A fixture `/var/lib/systemd/coredump` whose chain passes the System
/// scope checks (see `Env::system_owner_uid`).
fn coredump_dir(t: &FixtureTree) {
    t.mkdir("var/lib/systemd/coredump");
    let mut cur = t.dir.path().to_path_buf();
    fs::set_permissions(&cur, fs::Permissions::from_mode(0o755)).unwrap();
    for c in ["var", "lib", "systemd", "coredump"] {
        cur.push(c);
        fs::set_permissions(&cur, fs::Permissions::from_mode(0o755)).unwrap();
    }
}

fn core_delete(env: &Env, name: &str) -> PlannedAction {
    let p = PathBuf::from(format!("/var/lib/systemd/coredump/{name}"));
    PlannedAction {
        size: disk_usage(env, &p).bytes,
        fingerprint: fingerprint(env, &p).unwrap(),
        action: Action::Delete { path: p },
        scope: Scope::System,
        depends_on: None,
        depends_on_plan: None,
        reason: "test".into(),
    }
}

/// A fixture `rm` that runs the real one (on the root-joined path the
/// executor passes).
fn fixture_rm(t: &FixtureTree) {
    t.write_exec("usr/bin/rm", b"#!/bin/sh\nexec /usr/bin/rm \"$@\"\n");
}

/// A fixture program that appends its arguments to `log` and exits with
/// `status`, printing `output` first.
fn logging_program(t: &FixtureTree, name: &str, log: &Path, output: &str, status: i32) {
    let script = format!(
        "#!/bin/sh\necho \"$@\" >> '{}'\nprintf '%s' '{output}'\nexit {status}\n",
        log.display()
    );
    t.write_exec(&format!("usr/bin/{name}"), script.as_bytes());
}

#[test]
fn privileged_actions_run_in_one_batch() {
    let t = FixtureTree::new();
    let log = t.dir.path().join("calls.log");
    logging_program(&t, "flatpak", &log, "", 0);
    logging_program(&t, "pacman", &log, "removing foo...\n", 0);
    fixture_rm(&t);
    coredump_dir(&t);
    t.write("var/lib/systemd/coredump/core.x.1", b"dump");
    t.write("home/user/.config/foo/a", b"a");
    let env = t.env();
    let plan = plan_of(vec![
        planned(&env, trash("/home/user/.config/foo")),
        priv_cmd("flatpak", &["uninstall", "--system", "-y", "org.x.App"]),
        pkg_remove(&["foo"], &["1.0-1"]),
        core_delete(&env, "core.x.1"),
    ]);
    let pr = FakePrivilegedRunner::default();
    let report = execute_full(
        &env,
        &Config::default(),
        &plan,
        &FakeRunner::new(),
        &pr,
        Some(&esc()),
        &no_post_check,
    );
    for r in &report.results {
        assert!(r.ok, "{r:?}");
    }
    assert!(!report.escalation_failed);
    let calls = pr.calls.lock().unwrap().clone();
    assert_eq!(calls.len(), 1, "one escalation for the whole run");
    assert_eq!(step_count(&calls[0]), 3);
    assert_eq!(
        fs::read_to_string(&log).unwrap(),
        "uninstall --system -y org.x.App\n-Rns --noconfirm foo\n"
    );
    assert!(!env.sys("/var/lib/systemd/coredump/core.x.1").exists());
    assert!(!env.sys("/home/user/.config/foo").exists());
    assert_eq!(report.freed, plan.total_size);
    // Results stay in plan order; history ids name plan indices, the
    // privileged entries written right after the batch.
    for (i, r) in report.results.iter().enumerate() {
        assert_eq!(r.action, plan.actions[i].action);
        assert_eq!(r.history_id, format!("{}-{i}", report.batch));
    }
    let (hist, _) = read_history(&env).unwrap();
    let ids: Vec<&str> = hist.iter().map(|h| h.id.as_str()).collect();
    let b = &report.batch;
    assert_eq!(
        ids,
        [
            format!("{b}-1"),
            format!("{b}-2"),
            format!("{b}-3"),
            format!("{b}-0")
        ]
    );
    assert_eq!(hist[0].action, HistoryAction::PrivilegedCommand);
    assert_eq!(hist[0].path, Path::new("/usr/bin/flatpak"));
    assert_eq!(hist[1].action, HistoryAction::PackageRemove);
    assert_eq!(hist[2].action, HistoryAction::Delete);
    assert_eq!(
        hist[2].path,
        Path::new("/var/lib/systemd/coredump/core.x.1")
    );
    assert_eq!(hist[3].action, HistoryAction::Trash);
}

/// A runner whose escalation produced no output at all (cancelled or
/// denied): it records the script but runs nothing.
#[derive(Default)]
struct DeniedRunner {
    calls: std::sync::Mutex<usize>,
}

impl PrivilegedRunner for DeniedRunner {
    fn run(&self, _: &Escalation, _: &[u8]) -> Result<Vec<u8>, crate::error::CoreError> {
        *self.calls.lock().unwrap() += 1;
        Ok(Vec::new())
    }
}

#[test]
fn escalation_denied_marks_all_failed_and_skips_dependents() {
    let t = FixtureTree::new();
    let log = t.dir.path().join("calls.log");
    logging_program(&t, "pacman", &log, "", 0);
    logging_program(&t, "flatpak", &log, "", 0);
    t.write("home/user/.config/foo/a", b"a");
    let env = t.env();
    let mut leftover = planned(&env, trash("/home/user/.config/foo"));
    leftover.depends_on = Some(0);
    let plan = plan_of(vec![
        pkg_remove(&["foo"], &["1.0-1"]),
        leftover,
        priv_cmd("flatpak", &["uninstall", "--system", "-y", "org.x.App"]),
    ]);
    let pr = DeniedRunner::default();
    let report = execute_full(
        &env,
        &Config::default(),
        &plan,
        &FakeRunner::new(),
        &pr,
        Some(&esc()),
        // Nothing ran, so the package is still there.
        &|_, _, names| Ok(names.to_vec()),
    );
    assert_eq!(*pr.calls.lock().unwrap(), 1);
    assert!(report.escalation_failed);
    assert_eq!(
        report.results[0].error.as_deref(),
        Some("privilege escalation was cancelled or denied; still installed: foo")
    );
    assert_eq!(report.results[2].error.as_deref(), Some(ESCALATION_DENIED));
    assert!(report.results.iter().all(|r| !r.ok));
    let r = &report.results[1];
    assert!(!r.ok);
    assert!(
        r.error.as_deref().unwrap().starts_with("skipped: "),
        "{r:?}"
    );
    assert!(env.sys("/home/user/.config/foo/a").exists());
    assert!(!log.exists());
    assert_eq!(report.freed, 0);
    let (hist, _) = read_history(&env).unwrap();
    assert_eq!(hist.len(), 3);
    assert!(hist.iter().all(|h| !h.ok));
}

#[test]
fn no_escalation_tool_reports_and_flags() {
    let t = FixtureTree::new();
    let log = t.dir.path().join("calls.log");
    logging_program(&t, "pacman", &log, "", 0);
    t.write("home/user/.config/foo/a", b"a");
    let env = t.env();
    let plan = plan_of(vec![
        pkg_remove(&["foo"], &["1.0-1"]),
        planned(&env, trash("/home/user/.config/foo")),
    ]);
    let pr = FakePrivilegedRunner::default();
    let report = execute_full(
        &env,
        &Config::default(),
        &plan,
        &FakeRunner::new(),
        &pr,
        None,
        &no_post_check,
    );
    assert!(report.escalation_failed);
    assert!(pr.calls.lock().unwrap().is_empty());
    assert_eq!(
        report.results[0].error.as_deref(),
        Some("no way to get administrator rights (install systemd ≥ 256, polkit or sudo)")
    );
    // An independent user action still runs.
    assert!(report.results[1].ok, "{:?}", report.results[1]);
    assert!(!log.exists());

    // `execute_with` never escalates.
    let t = FixtureTree::new();
    logging_program(&t, "pacman", &log, "", 0);
    let env = t.env();
    let plan = plan_of(vec![pkg_remove(&["foo"], &["1.0-1"])]);
    let report = execute_with(&env, &Config::default(), &plan, &FakeRunner::new());
    assert!(report.escalation_failed);
    assert_eq!(
        report.results[0].error.as_deref(),
        Some("no way to get administrator rights (install systemd ≥ 256, polkit or sudo)")
    );
}

#[test]
fn system_delete_verified_before_batch() {
    let t = FixtureTree::new();
    fixture_rm(&t);
    coredump_dir(&t);
    t.write("var/lib/systemd/coredump/core.a.1", b"a");
    t.write("var/lib/systemd/coredump/core.b.1", b"b");
    t.write("etc/precious", b"keep me");
    let env = t.env();
    let plan = plan_of(vec![
        core_delete(&env, "core.a.1"),
        core_delete(&env, "core.b.1"),
    ]);
    // core.b.1 is swapped for a symlink to a precious file after the scan.
    let b = env.sys("/var/lib/systemd/coredump/core.b.1");
    fs::remove_file(&b).unwrap();
    std::os::unix::fs::symlink(env.sys("/etc/precious"), &b).unwrap();
    let pr = FakePrivilegedRunner::default();
    let report = execute_full(
        &env,
        &Config::default(),
        &plan,
        &FakeRunner::new(),
        &pr,
        Some(&esc()),
        &no_post_check,
    );
    assert!(report.results[0].ok, "{:?}", report.results[0]);
    assert!(!report.results[1].ok);
    assert!(!report.escalation_failed);
    let calls = pr.calls.lock().unwrap().clone();
    assert_eq!(calls.len(), 1);
    assert_eq!(step_count(&calls[0]), 1, "the swapped file is left out");
    let script = String::from_utf8_lossy(&calls[0]);
    assert!(script.contains("core.a.1"));
    assert!(!script.contains("core.b.1"));
    assert!(script.contains("'-f' '--'"), "{script}");
    assert!(!env.sys("/var/lib/systemd/coredump/core.a.1").exists());
    assert!(fs::symlink_metadata(&b).unwrap().file_type().is_symlink());
    assert_eq!(fs::read(env.sys("/etc/precious")).unwrap(), b"keep me");
}

#[test]
fn empty_batch_never_escalates() {
    let t = FixtureTree::new();
    fixture_rm(&t);
    coredump_dir(&t);
    t.write("var/lib/systemd/coredump/core.a.1", b"a");
    let env = t.env();
    let plan = plan_of(vec![
        core_delete(&env, "core.a.1"),
        // Not in a trusted directory: not installed.
        priv_cmd("no-such-tool", &["x"]),
    ]);
    fs::remove_file(env.sys("/var/lib/systemd/coredump/core.a.1")).unwrap();
    let pr = DeniedRunner::default();
    let report = execute_full(
        &env,
        &Config::default(),
        &plan,
        &FakeRunner::new(),
        &pr,
        Some(&esc()),
        &no_post_check,
    );
    assert_eq!(*pr.calls.lock().unwrap(), 0);
    assert!(!report.escalation_failed);
    assert!(!report.results[0].ok);
    assert_eq!(
        report.results[1].error.as_deref(),
        Some("no-such-tool is not installed")
    );
    // No escalation tool and nothing to escalate for: not a failure.
    let report = execute_full(
        &env,
        &Config::default(),
        &plan_of(vec![priv_cmd("no-such-tool", &[])]),
        &FakeRunner::new(),
        &pr,
        None,
        &no_post_check,
    );
    assert!(!report.escalation_failed);
}

#[test]
fn privileged_step_failure_reports_last_output_line() {
    let t = FixtureTree::new();
    let log = t.dir.path().join("calls.log");
    logging_program(
        &t,
        "pacman",
        &log,
        "checking...\nerror: target not found: foo\n",
        1,
    );
    logging_program(&t, "flatpak", &log, "", 3);
    logging_program(&t, "true", &log, "", 0);
    let env = t.env();
    let plan = plan_of(vec![
        pkg_remove(&["foo"], &["1.0-1"]),
        priv_cmd("flatpak", &["x"]),
        priv_cmd("true", &[]),
    ]);
    let report = execute_full(
        &env,
        &Config::default(),
        &plan,
        &FakeRunner::new(),
        &FakePrivilegedRunner::default(),
        Some(&esc()),
        &|_, _, names| Ok(names.to_vec()),
    );
    assert_eq!(
        report.results[0].error.as_deref(),
        Some("pacman exited with status 1: error: target not found: foo; still installed: foo")
    );
    assert_eq!(
        report.results[1].error.as_deref(),
        Some("exited with status 3")
    );
    assert!(report.results[2].ok);
    assert!(!report.escalation_failed);
    assert_eq!(report.freed, 0);
}

#[test]
fn batch_error_fails_privileged_actions_without_escalation_failure() {
    let t = FixtureTree::new();
    let log = t.dir.path().join("calls.log");
    logging_program(&t, "flatpak", &log, "", 0);
    let env = t.env();
    let plan = plan_of(vec![
        priv_cmd("flatpak", &["bad\0arg"]),
        priv_cmd("flatpak", &["fine"]),
    ]);
    let pr = DeniedRunner::default();
    let report = execute_full(
        &env,
        &Config::default(),
        &plan,
        &FakeRunner::new(),
        &pr,
        Some(&esc()),
        &no_post_check,
    );
    assert_eq!(*pr.calls.lock().unwrap(), 0);
    assert!(!report.escalation_failed);
    for r in &report.results {
        assert!(!r.ok);
        assert!(r.error.as_deref().unwrap().contains("NUL byte"), "{r:?}");
    }
}

#[test]
fn privileged_action_with_dependency_is_skipped() {
    let t = FixtureTree::new();
    let log = t.dir.path().join("calls.log");
    logging_program(&t, "flatpak", &log, "", 0);
    t.write("home/user/.config/foo/a", b"a");
    let env = t.env();
    let mut cmd = priv_cmd("flatpak", &["x"]);
    cmd.depends_on = Some(0);
    let plan = plan_of(vec![planned(&env, trash("/home/user/.config/foo")), cmd]);
    let pr = DeniedRunner::default();
    let report = execute_full(
        &env,
        &Config::default(),
        &plan,
        &FakeRunner::new(),
        &pr,
        Some(&esc()),
        &no_post_check,
    );
    assert!(report.results[0].ok);
    assert!(!report.results[1].ok);
    assert!(
        report.results[1]
            .error
            .as_deref()
            .unwrap()
            .starts_with("skipped: "),
        "{:?}",
        report.results[1]
    );
    assert_eq!(*pr.calls.lock().unwrap(), 0);
}

#[test]
fn package_remove_post_check_still_installed() {
    let t = FixtureTree::new();
    let log = t.dir.path().join("calls.log");
    logging_program(&t, "pacman", &log, "", 0);
    let env = t.env();
    let plan = plan_of(vec![
        pkg_remove(&["foo", "bar"], &["1.0-1", "2.0-1"]),
        pkg_remove(&["baz"], &["3.0-1"]),
    ]);
    let seen = std::sync::Mutex::new(Vec::new());
    let post_check = |_: &Env, source: SourceKind, names: &[String]| {
        seen.lock().unwrap().push((source, names.to_vec()));
        Ok(names.iter().filter(|n| *n == "foo").cloned().collect())
    };
    let report = execute_full(
        &env,
        &Config::default(),
        &plan,
        &FakeRunner::new(),
        &FakePrivilegedRunner::default(),
        Some(&esc()),
        &post_check,
    );
    assert!(!report.results[0].ok);
    assert_eq!(
        report.results[0].error.as_deref(),
        Some("removed: bar; still installed: foo")
    );
    assert!(report.results[1].ok, "{:?}", report.results[1]);
    assert_eq!(report.freed, 100);
    assert_eq!(
        *seen.lock().unwrap(),
        [
            (SourceKind::Pacman, vec!["foo".to_string(), "bar".into()]),
            (SourceKind::Pacman, vec!["baz".to_string()]),
        ]
    );
    let (hist, _) = read_history(&env).unwrap();
    assert!(!hist[0].ok);
    assert_eq!(
        hist[0].error.as_deref(),
        Some("removed: bar; still installed: foo")
    );
    assert_eq!(
        hist[0].packages.as_deref(),
        Some(&[("bar".to_string(), "2.0-1".to_string())][..])
    );
}

#[test]
fn history_records_packages() {
    let t = FixtureTree::new();
    let log = t.dir.path().join("calls.log");
    logging_program(&t, "pacman", &log, "", 0);
    logging_program(&t, "flatpak", &log, "", 0);
    let env = t.env();
    let plan = plan_of(vec![
        pkg_remove(&["foo", "libfoo"], &["1.0-1", "2.1-3"]),
        priv_cmd("flatpak", &["uninstall", "--system", "-y", "org.x.App"]),
    ]);
    let report = execute_full(
        &env,
        &Config::default(),
        &plan,
        &FakeRunner::new(),
        &FakePrivilegedRunner::default(),
        Some(&esc()),
        &no_post_check,
    );
    assert!(report.results.iter().all(|r| r.ok), "{report:?}");
    let (hist, _) = read_history(&env).unwrap();
    assert_eq!(hist[0].action, HistoryAction::PackageRemove);
    assert_eq!(hist[0].path, Path::new("/usr/bin/pacman"));
    assert_eq!(
        hist[0].packages.as_deref(),
        Some(
            &[
                ("foo".to_string(), "1.0-1".to_string()),
                ("libfoo".into(), "2.1-3".into())
            ][..]
        )
    );
    assert_eq!(
        hist[0].argv.as_deref(),
        Some(
            &[
                "-Rns".to_string(),
                "--noconfirm".into(),
                "foo".into(),
                "libfoo".into()
            ][..]
        )
    );
    assert_eq!(hist[0].size, 100);
    assert_eq!(hist[1].action, HistoryAction::PrivilegedCommand);
    assert_eq!(hist[1].path, Path::new("/usr/bin/flatpak"));
    assert_eq!(hist[1].packages, None);
    assert_eq!(
        hist[1].argv.as_deref(),
        Some(
            &[
                "uninstall".to_string(),
                "--system".into(),
                "-y".into(),
                "org.x.App".into()
            ][..]
        )
    );
    let text = fs::read_to_string(env.state_home.join("crumbs/history.jsonl")).unwrap();
    assert!(text.contains("\"action\":\"package-remove\""), "{text}");
    assert!(text.contains("\"packages\":[[\"foo\",\"1.0-1\"]"), "{text}");
    assert!(text.contains("\"action\":\"privileged-command\""), "{text}");
}

#[test]
fn execute_with_on_fake_root_never_escalates() {
    let t = FixtureTree::new();
    let log = t.dir.path().join("calls.log");
    logging_program(&t, "pacman", &log, "", 0);
    // An escalation program detection would otherwise pick.
    let esc_log = t.dir.path().join("pkexec.log");
    logging_program(&t, "pkexec", &esc_log, "", 0);
    let env = t.env();
    let plan = plan_of(vec![pkg_remove(&["foo"], &["1.0-1"])]);
    let runner = FakeRunner::new();
    let report = execute_with(&env, &Config::default(), &plan, &runner);
    assert_eq!(report.results[0].error.as_deref(), Some(NO_ESCALATION));
    assert!(report.escalation_failed);
    assert!(!esc_log.exists(), "the escalation program ran");
    assert!(!log.exists());
    assert!(runner.calls().is_empty(), "{:?}", runner.calls());
}

/// A runner that hands back canned stdout without running anything.
struct CannedRunner(Vec<u8>);

impl PrivilegedRunner for CannedRunner {
    fn run(&self, _: &Escalation, _: &[u8]) -> Result<Vec<u8>, crate::error::CoreError> {
        Ok(self.0.clone())
    }
}

#[test]
fn partial_markers_give_mixed_results() {
    let t = FixtureTree::new();
    let log = t.dir.path().join("calls.log");
    logging_program(&t, "flatpak", &log, "", 0);
    let env = t.env();
    let plan = plan_of(vec![
        priv_cmd("flatpak", &["a"]),
        priv_cmd("flatpak", &["b"]),
        priv_cmd("flatpak", &["c"]),
    ]);
    let report = execute_full(
        &env,
        &Config::default(),
        &plan,
        &FakeRunner::new(),
        &CannedRunner(b"|done\n@@crumbs 0 0\n|half\n".to_vec()),
        Some(&esc()),
        &no_post_check,
    );
    assert!(report.results[0].ok, "{:?}", report.results[0]);
    assert!(!report.results[1].ok);
    assert_eq!(report.results[1].error.as_deref(), Some(STOPPED_DURING));
    assert_eq!(report.results[2].error.as_deref(), Some(STOPPED_BEFORE));
    // The shell ran as root and was stopped: rights were not missing.
    assert!(!report.escalation_failed);
    assert!(report.batch_stopped);
    assert!(report.results.iter().all(|r| !r.rights_missing));
    // The history records what ran, and what did not.
    let (hist, _) = read_history(&env).unwrap();
    assert_eq!(hist.len(), 3);
    let oks: Vec<bool> = hist.iter().map(|h| h.ok).collect();
    assert_eq!(oks, [true, false, false]);
}

#[test]
fn no_markers_is_a_denial() {
    let t = FixtureTree::new();
    let log = t.dir.path().join("calls.log");
    logging_program(&t, "flatpak", &log, "", 0);
    let env = t.env();
    let plan = plan_of(vec![
        priv_cmd("flatpak", &["a"]),
        priv_cmd("flatpak", &["b"]),
    ]);
    let report = execute_full(
        &env,
        &Config::default(),
        &plan,
        &FakeRunner::new(),
        &CannedRunner(b"noise\n|not a marker\n".to_vec()),
        Some(&esc()),
        &no_post_check,
    );
    assert!(report.escalation_failed);
    assert!(!report.batch_stopped);
    for r in &report.results {
        assert_eq!(r.error.as_deref(), Some(ESCALATION_DENIED));
        assert!(r.rights_missing);
    }
}

#[test]
fn stopped_package_removal_still_records_what_went() {
    let t = FixtureTree::new();
    let log = t.dir.path().join("calls.log");
    logging_program(&t, "flatpak", &log, "", 0);
    logging_program(&t, "pacman", &log, "", 0);
    let env = t.env();
    let plan = plan_of(vec![
        priv_cmd("flatpak", &["a"]),
        pkg_remove(&["foo", "bar"], &["1.0-1", "2.0-1"]),
    ]);
    // `bar` is gone, `foo` is still there: the step stopped part-way.
    let post = |_: &Env, _: SourceKind, _: &[String]| Ok(vec!["foo".to_string()]);
    let report = execute_full(
        &env,
        &Config::default(),
        &plan,
        &FakeRunner::new(),
        &CannedRunner(b"@@crumbs 0 0\n".to_vec()),
        Some(&esc()),
        &post,
    );
    let err = report.results[1].error.as_deref().unwrap();
    assert!(err.starts_with(STOPPED_DURING), "{err}");
    assert!(err.contains("removed: bar"), "{err}");
    assert!(err.contains("still installed: foo"), "{err}");
    assert!(!report.escalation_failed);
    let (hist, _) = read_history(&env).unwrap();
    let text = serde_json::to_string(&hist[1]).unwrap();
    assert!(
        text.contains("\"packages\":[[\"bar\",\"2.0-1\"]]"),
        "{text}"
    );
}

#[test]
fn too_large_batch_fails_without_escalation_failure() {
    let t = FixtureTree::new();
    let log = t.dir.path().join("calls.log");
    logging_program(&t, "flatpak", &log, "", 0);
    let env = t.env();
    let big = "x".repeat(130 * 1024);
    let plan = plan_of(vec![
        priv_cmd("flatpak", &["a"]),
        priv_cmd("flatpak", &[&big]),
    ]);
    let pr = DeniedRunner::default();
    let report = execute_full(
        &env,
        &Config::default(),
        &plan,
        &FakeRunner::new(),
        &pr,
        Some(&esc()),
        &no_post_check,
    );
    assert_eq!(*pr.calls.lock().unwrap(), 0);
    assert!(!report.escalation_failed);
    for r in &report.results {
        assert!(!r.ok);
        assert!(
            r.error.as_deref().unwrap().starts_with("too many items"),
            "{r:?}"
        );
    }
}

#[test]
fn dependent_of_package_failing_post_check_is_skipped() {
    let t = FixtureTree::new();
    let log = t.dir.path().join("calls.log");
    logging_program(&t, "pacman", &log, "", 0);
    t.write("home/user/.config/foo/a", b"a");
    let env = t.env();
    let mut leftover = planned(&env, trash("/home/user/.config/foo"));
    leftover.depends_on = Some(0);
    let plan = plan_of(vec![pkg_remove(&["foo"], &["1.0-1"]), leftover]);
    let report = execute_full(
        &env,
        &Config::default(),
        &plan,
        &FakeRunner::new(),
        &FakePrivilegedRunner::default(),
        Some(&esc()),
        &|_, _, names| Ok(names.to_vec()),
    );
    assert_eq!(
        report.results[0].error.as_deref(),
        Some("still installed: foo")
    );
    assert!(!report.results[1].ok);
    assert!(
        report.results[1]
            .error
            .as_deref()
            .unwrap()
            .starts_with("skipped: ")
    );
    assert!(env.sys("/home/user/.config/foo/a").exists());
}

#[test]
fn failed_package_step_reports_what_was_removed() {
    let t = FixtureTree::new();
    let log = t.dir.path().join("calls.log");
    logging_program(
        &t,
        "pacman",
        &log,
        "error: failed to commit transaction\n",
        1,
    );
    let env = t.env();
    let plan = plan_of(vec![pkg_remove(&["a", "b", "c"], &["1", "2", "3"])]);
    let still = |_: &Env, _: SourceKind, _: &[String]| Ok(vec!["c".to_string()]);
    let report = execute_full(
        &env,
        &Config::default(),
        &plan,
        &FakeRunner::new(),
        &FakePrivilegedRunner::default(),
        Some(&esc()),
        &still,
    );
    assert!(!report.results[0].ok);
    assert_eq!(
        report.results[0].error.as_deref(),
        Some(
            "pacman exited with status 1: error: failed to commit transaction; \
             removed: a, b; still installed: c"
        )
    );
    let (hist, _) = read_history(&env).unwrap();
    assert_eq!(
        hist[0].packages.as_deref(),
        Some(&[("a".to_string(), "1".to_string()), ("b".into(), "2".into())][..])
    );

    // A missing marker: the post-check still says what is gone.
    let t = FixtureTree::new();
    logging_program(&t, "pacman", &log, "", 0);
    let env = t.env();
    let none_left = |_: &Env, _: SourceKind, _: &[String]| Ok(Vec::new());
    let report = execute_full(
        &env,
        &Config::default(),
        &plan_of(vec![pkg_remove(&["a"], &["1"])]),
        &FakeRunner::new(),
        &DeniedRunner::default(),
        Some(&esc()),
        &none_left,
    );
    assert!(!report.results[0].ok);
    assert!(report.escalation_failed);
    assert_eq!(
        report.results[0].error.as_deref(),
        Some("privilege escalation was cancelled or denied; removed: a")
    );
}

/// Without a real post-check (`execute`/`execute_with` until removal is
/// wired) a failed package step claims nothing about what was removed.
#[test]
fn default_post_check_claims_nothing_on_failure() {
    let t = FixtureTree::new();
    let log = t.dir.path().join("calls.log");
    logging_program(&t, "pacman", &log, "error: boom\n", 1);
    let env = t.env();
    let plan = plan_of(vec![pkg_remove(&["a", "b"], &["1", "2"])]);
    let run = |pr: &dyn PrivilegedRunner| {
        run_one(
            &env,
            &Config::default(),
            &plan,
            &FakeRunner::new(),
            pr,
            Some(&esc()),
            None,
        )
    };
    let report = run(&FakePrivilegedRunner::default());
    assert_eq!(
        report.results[0].error.as_deref(),
        Some("pacman exited with status 1: error: boom")
    );
    let report = run(&DeniedRunner::default());
    assert_eq!(report.results[0].error.as_deref(), Some(ESCALATION_DENIED));
    let (hist, _) = read_history(&env).unwrap();
    assert!(hist.iter().all(|h| h.packages.is_none()), "{hist:?}");

    // A step that succeeded is trusted: every package is recorded.
    logging_program(&t, "pacman", &log, "", 0);
    let report = run(&FakePrivilegedRunner::default());
    assert!(report.results[0].ok, "{:?}", report.results[0]);
    let (hist, _) = read_history(&env).unwrap();
    assert_eq!(
        hist.last().unwrap().packages.as_deref(),
        Some(&[("a".to_string(), "1".to_string()), ("b".into(), "2".into())][..])
    );
}

#[test]
fn unverifiable_removal_is_not_ok_and_records_nothing() {
    let t = FixtureTree::new();
    let log = t.dir.path().join("calls.log");
    logging_program(&t, "pacman", &log, "", 0);
    t.write("home/user/.config/foo/a", b"a");
    let env = t.env();
    let mut leftover = planned(&env, trash("/home/user/.config/foo"));
    leftover.depends_on = Some(0);
    let plan = plan_of(vec![pkg_remove(&["foo"], &["1.0-1"]), leftover]);
    let report = execute_full(
        &env,
        &Config::default(),
        &plan,
        &FakeRunner::new(),
        &FakePrivilegedRunner::default(),
        Some(&esc()),
        &|_, _, _| Err("cannot read the pacman database".into()),
    );
    assert!(!report.results[0].ok);
    assert_eq!(
        report.results[0].error.as_deref(),
        Some("could not verify removal: cannot read the pacman database")
    );
    assert!(
        report.results[1]
            .error
            .as_deref()
            .unwrap()
            .starts_with("skipped: ")
    );
    assert!(env.sys("/home/user/.config/foo/a").exists());
    let (hist, _) = read_history(&env).unwrap();
    assert!(hist.iter().all(|h| h.packages.is_none()), "{hist:?}");
}

fn app_plan(app_id: &str, actions: Vec<PlannedAction>) -> Plan {
    let mut p = plan_of(actions);
    p.app_id = Some(app_id.into());
    p
}

#[test]
fn execute_many_uses_one_batch_for_every_plan() {
    let t = FixtureTree::new();
    let log = t.dir.path().join("calls.log");
    logging_program(&t, "pacman", &log, "", 0);
    t.write("home/user/.config/foo/a", b"a");
    t.write("home/user/.config/bar/a", b"a");
    let env = t.env();
    let mut foo_left = planned(&env, trash("/home/user/.config/foo"));
    foo_left.depends_on = Some(0);
    let mut bar_left = planned(&env, trash("/home/user/.config/bar"));
    bar_left.depends_on = Some(0);
    let plans = [
        app_plan("pacman:foo", vec![pkg_remove(&["foo"], &["1-1"]), foo_left]),
        app_plan("pacman:bar", vec![pkg_remove(&["bar"], &["2-1"]), bar_left]),
    ];
    let pr = FakePrivilegedRunner::default();
    let reports = execute_many(
        &env,
        &Config::default(),
        &plans,
        &FakeRunner::new(),
        &pr,
        Some(&esc()),
        &no_post_check,
    );
    assert_eq!(reports.len(), 2);
    let calls = pr.calls.lock().unwrap().clone();
    assert_eq!(calls.len(), 1, "one escalation for the whole run");
    assert_eq!(step_count(&calls[0]), 2);
    assert_eq!(
        fs::read_to_string(&log).unwrap(),
        "-Rns --noconfirm foo\n-Rns --noconfirm bar\n"
    );
    for r in &reports {
        assert!(r.results.iter().all(|x| x.ok), "{r:?}");
        assert!(!r.escalation_failed);
    }
    assert_ne!(reports[0].batch, reports[1].batch);
    assert!(!env.sys("/home/user/.config/bar").exists());
    // History keeps each plan's batch id and app.
    let (hist, _) = read_history(&env).unwrap();
    assert_eq!(hist.len(), 4);
    for h in &hist {
        let (batch, app) = if h.batch == reports[0].batch {
            (&reports[0].batch, "pacman:foo")
        } else {
            (&reports[1].batch, "pacman:bar")
        };
        assert_eq!(&h.batch, batch);
        assert_eq!(h.app_id.as_deref(), Some(app), "{h:?}");
    }
    // A later run gets a later batch id than both.
    let next: u128 = crate::history::next_batch_id(&env).parse().unwrap();
    assert!(next > reports[1].batch.parse::<u128>().unwrap());
}

/// An action an earlier plan's action covered waits for it: it is skipped,
/// naming the failed cover, and never run; one whose cover succeeded runs.
#[test]
fn cross_plan_gate_skips_when_cover_failed() {
    let t = FixtureTree::new();
    t.write("home/user/.config/shared/a", b"a");
    t.write("home/user/.config/ok/a", b"a");
    t.write("home/user/.config/dep/a", b"a");
    t.write("home/user/.config/dep2/a", b"a");
    let env = t.env();
    let shared = planned(&env, trash("/home/user/.config/shared"));
    let ok = planned(&env, trash("/home/user/.config/ok"));
    let mut dep = planned(&env, trash("/home/user/.config/dep"));
    dep.depends_on_plan = Some((0, 0));
    let mut dep2 = planned(&env, trash("/home/user/.config/dep2"));
    dep2.depends_on_plan = Some((0, 1));
    // The cover vanishes after the scan, so its action fails.
    fs::remove_dir_all(env.sys("/home/user/.config/shared")).unwrap();
    let plans = [
        app_plan("pacman:foo", vec![shared, ok]),
        app_plan("pacman:bar", vec![dep, dep2]),
    ];
    let reports = execute_many(
        &env,
        &Config::default(),
        &plans,
        &FakeRunner::new(),
        &FakePrivilegedRunner::default(),
        Some(&esc()),
        &no_post_check,
    );
    assert!(!reports[0].results[0].ok);
    assert!(reports[0].results[1].ok);
    let skipped = &reports[1].results[0];
    assert!(!skipped.ok);
    assert!(
        !skipped.rights_missing,
        "a skipped dependent is never listed"
    );
    assert_eq!(
        skipped.error.as_deref(),
        Some("skipped: ~/.config/shared was not removed (it covered this action)")
    );
    assert!(env.sys("/home/user/.config/dep").exists(), "must not run");
    assert!(reports[1].results[1].ok);
    assert!(!env.sys("/home/user/.config/dep2").exists());
}

#[test]
fn execute_many_denied_fails_every_plan() {
    let t = FixtureTree::new();
    let log = t.dir.path().join("calls.log");
    logging_program(&t, "pacman", &log, "", 0);
    let env = t.env();
    let plans = [
        app_plan("pacman:foo", vec![pkg_remove(&["foo"], &["1-1"])]),
        // A plan with nothing privileged is not flagged.
        app_plan("x", Vec::new()),
        app_plan("pacman:bar", vec![priv_cmd("pacman", &["-Rn", "bar"])]),
    ];
    let pr = DeniedRunner::default();
    let reports = execute_many(
        &env,
        &Config::default(),
        &plans,
        &FakeRunner::new(),
        &pr,
        Some(&esc()),
        &|_, _, names| Ok(names.to_vec()),
    );
    assert_eq!(*pr.calls.lock().unwrap(), 1);
    assert!(reports[0].escalation_failed && reports[2].escalation_failed);
    assert!(!reports[1].escalation_failed);
    assert_eq!(
        reports[0].results[0].error.as_deref(),
        Some("privilege escalation was cancelled or denied; still installed: foo")
    );
    assert_eq!(
        reports[2].results[0].error.as_deref(),
        Some(ESCALATION_DENIED)
    );
}
