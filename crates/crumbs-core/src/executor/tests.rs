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
        size_known: true,
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
fn running_detects_script_launched_electron_app() {
    let t = FixtureTree::new();
    let env = t.env();
    let mut app = App::new(SourceKind::Unmanaged, "claude");
    app.exec_names = vec!["claude".into()];
    app.program = Some("/usr/lib/claude/claude".into());
    app.program_link = Some("/usr/bin/claude".into());
    // A launcher script run by the shell: argv1 is the script.
    t.symlink("proc/30/exe", "/usr/bin/bash");
    t.write("proc/30/cmdline", b"/bin/sh\0/usr/bin/claude\0--flag\0");
    // An interpreter running the canonical program.
    t.symlink("proc/31/exe", "/usr/bin/node");
    t.write("proc/31/cmdline", b"node\0/usr/lib/claude/claude\0");
    // A generic interpreter running something else, or the name only as a
    // later argument, never matches.
    t.symlink("proc/32/exe", "/usr/bin/node");
    t.write("proc/32/cmdline", b"node\0/usr/lib/other/cli.js\0");
    t.write("proc/33/cmdline", b"node\0x.js\0/usr/bin/claude\0");
    t.write("proc/34/cmdline", b"/bin/sh\0/home/user/claude\0");
    assert_eq!(running_processes(&env, &app), [30, 31]);
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
    if crate::testing::running_as_root() {
        eprintln!("skipped: temp cleanup is not offered as root");
        return;
    }
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
    if crate::testing::running_as_root() {
        eprintln!("skipped: temp cleanup is not offered as root");
        return;
    }
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
        size_known: true,
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
    if crate::testing::running_as_root() {
        eprintln!("skipped: temp cleanup is not offered as root");
        return;
    }
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
        size_known: true,
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
        size_known: true,
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

/// Denies like [`DeniedRunner`], with what the escalation wrote on stderr.
struct DeniedWithStderr;

impl PrivilegedRunner for DeniedWithStderr {
    fn run(&self, _: &Escalation, _: &[u8]) -> Result<Vec<u8>, crate::error::CoreError> {
        Ok(Vec::new())
    }

    fn run_captured(
        &self,
        _: &Escalation,
        _: &[u8],
    ) -> Result<crate::privilege::batch::PrivilegedRun, crate::error::CoreError> {
        Ok(crate::privilege::batch::PrivilegedRun {
            stdout: Vec::new(),
            stderr: b"polkit: not authorized\n".to_vec(),
        })
    }
}

#[test]
fn denied_batch_keeps_the_escalation_stderr_and_a_run_one_does_not() {
    let t = FixtureTree::new();
    let log = t.dir.path().join("calls.log");
    logging_program(&t, "flatpak", &log, "", 0);
    let env = t.env();
    let plan = plan_of(vec![priv_cmd(
        "flatpak",
        &["uninstall", "--system", "-y", "org.x.App"],
    )]);
    let run = |pr: &dyn PrivilegedRunner| {
        execute_full(
            &env,
            &Config::default(),
            &plan,
            &FakeRunner::new(),
            pr,
            Some(&esc()),
            &|_, _, _| Ok(Vec::new()),
        )
    };
    let report = run(&DeniedWithStderr);
    assert!(report.escalation_failed);
    assert_eq!(report.escalation_output, b"polkit: not authorized\n");
    // Not in the JSON.
    let json = serde_json::to_string(&report).unwrap();
    assert!(!json.contains("polkit"), "{json}");
    // Markers came back: nothing kept, whatever was on stderr.
    let report = run(&FakePrivilegedRunner::with_interaction(
        crate::privilege::Interaction::NoTerminal,
    ));
    assert!(!report.escalation_failed, "{report:?}");
    assert!(report.escalation_output.is_empty());
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

/// A mountinfo line for `mount` (a root-joined path in fixtures).
fn mountinfo_line(mount: &Path) -> String {
    format!(
        "36 35 98:0 /mnt1 {} rw,noatime master:1 - ext3 /dev/root rw\n",
        mount.display()
    )
}

/// A fake root where `/opt/inky` and `/opt/blot` pass every SystemTree
/// check: a 0755 chain owned by the test user, a pacman database that owns
/// only `/opt/foo`, a mountinfo with nothing below `/opt`, and fixture `sh`,
/// `readlink` and `rm` that run the real ones.
fn system_tree_fixture() -> (FixtureTree, Env) {
    let t = FixtureTree::new();
    fixture_rm(&t);
    t.write_exec("usr/bin/sh", b"#!/bin/sh\nexec /bin/sh \"$@\"\n");
    t.write_exec(
        "usr/bin/readlink",
        b"#!/bin/sh\nexec /usr/bin/readlink \"$@\"\n",
    );
    t.pacman_pkg("foo", "1-1", Some(1), &["opt/foo/", "opt/foo/x"]);
    t.write("opt/inky/a", b"a");
    t.write("opt/inky/sub/b", b"b");
    t.write("opt/blot/c", b"c");
    t.write(
        "proc/self/mountinfo",
        mountinfo_line(Path::new("/")).as_bytes(),
    );
    for rel in ["", "opt"] {
        fs::set_permissions(t.dir.path().join(rel), fs::Permissions::from_mode(0o755)).unwrap();
    }
    let mut env = t.env();
    env.set_system_owner_uid(env.uid);
    (t, env)
}

fn tree_delete(env: &Env, p: &str) -> PlannedAction {
    let mut pa = planned(env, delete(p));
    pa.scope = Scope::SystemTree;
    pa
}

fn run_tree(env: &Env, plan: &Plan) -> (ExecReport, Vec<Vec<u8>>) {
    let pr = FakePrivilegedRunner::default();
    let report = execute_full(
        env,
        &Config::default(),
        plan,
        &FakeRunner::new(),
        &pr,
        Some(&esc()),
        &no_post_check,
    );
    let calls = pr.calls.lock().unwrap().clone();
    (report, calls)
}

/// The user's mount namespace, as the guard compares it; `None` (with a
/// message, so the caller skips) when `/proc` cannot be read here.
fn own_mount_ns() -> Option<std::ffi::OsString> {
    match fs::read_link("/proc/self/ns/mnt") {
        Ok(ns) => Some(ns.into_os_string()),
        Err(e) => {
            eprintln!("skipped: /proc/self/ns/mnt is unreadable here ({e})");
            None
        }
    }
}

/// The guard script a tree step in `env` runs, from the real builder.
fn guard_script(env: &Env) -> std::ffi::OsString {
    super::tree_script(
        &env.sys("/var/lib/pacman/db.lck"),
        &env.sys("/usr/bin/readlink"),
        &env.sys("/usr/bin/rm"),
    )
}

#[test]
fn system_tree_delete_runs_one_rm_rf_step() {
    let Some(ns) = own_mount_ns() else { return };
    let (_t, env) = system_tree_fixture();
    let plan = plan_of(vec![
        tree_delete(&env, "/opt/inky"),
        tree_delete(&env, "/opt/blot"),
    ]);
    let (report, calls) = run_tree(&env, &plan);
    for r in &report.results {
        assert!(r.ok, "{r:?}");
    }
    assert_eq!(calls.len(), 1, "one escalation for the whole run");
    assert_eq!(step_count(&calls[0]), 2);
    let script = String::from_utf8_lossy(&calls[0]);
    let guard = guard_script(&env);
    // The guard is one quoted word of the step, so its quotes are escaped.
    let quoted = String::from_utf8_lossy(&crate::privilege::batch::sh_quote(&guard)).into_owned();
    assert!(script.contains(&quoted), "{script}");
    assert!(script.contains("'crumbs-rm'"), "{script}");
    assert!(
        script.contains(&format!("'{}'", ns.to_string_lossy())),
        "{script}"
    );
    assert!(
        script.contains("'inky'") && script.contains("'blot'"),
        "{script}"
    );
    assert!(!env.sys("/opt/inky").exists());
    assert!(!env.sys("/opt/blot").exists());
    assert_eq!(report.freed, plan.total_size);
}

#[test]
fn system_tree_guard_text() {
    let env = FixtureTree::new().env();
    let lock = env.sys("/var/lib/pacman/db.lck").display().to_string();
    let readlink = env.sys("/usr/bin/readlink");
    let rm = env.sys("/usr/bin/rm");
    // Spelled out once here; every other test uses the builder.
    let want = format!(
        "{{ [ -e '{lock}' ] || [ -L '{lock}' ]; }} && {{ echo \"pacman is busy (database lock \
         present); not deleting\" >&2; exit 97; }}; test \"$('{}' /proc/self/ns/mnt)\" = \"$1\" || {{ echo \"mount \
         namespace differs; not deleting\" >&2; exit 97; }}; exec '{}' -rf --one-file-system \
         -- \"$2\"",
        readlink.display(),
        rm.display()
    );
    assert_eq!(guard_script(&env), std::ffi::OsString::from(want));
}

#[test]
fn system_tree_step_args_arrive_byte_exact() {
    let Some(ns) = own_mount_ns() else { return };
    let (t, env) = system_tree_fixture();
    // An `sh` that logs each argument, NUL-terminated, then runs the real one.
    let log = t.dir.path().join("sh.log");
    let sh = format!(
        "#!/bin/sh\nprintf '%s\\0' \"$@\" >> '{}'\nexec /bin/sh \"$@\"\n",
        log.display()
    );
    t.write_exec("usr/bin/sh", sh.as_bytes());
    let plan = plan_of(vec![tree_delete(&env, "/opt/inky")]);
    let (report, _) = run_tree(&env, &plan);
    assert!(report.results[0].ok, "{:?}", report.results[0]);
    let mut expected: Vec<u8> = Vec::new();
    for arg in [
        std::ffi::OsString::from("-c"),
        guard_script(&env),
        "crumbs-rm".into(),
        ns,
        env.sys("/opt/inky").into_os_string(),
    ] {
        expected.extend_from_slice(arg.as_encoded_bytes());
        expected.push(0);
    }
    assert_eq!(fs::read(&log).unwrap(), expected);
}

#[test]
fn system_tree_delete_refused_when_namespace_differs() {
    let (_t, mut env) = system_tree_fixture();
    env.set_mount_ns(Some("mnt:[1]".into()));
    let plan = plan_of(vec![tree_delete(&env, "/opt/inky")]);
    let (report, calls) = run_tree(&env, &plan);
    let r = &report.results[0];
    assert!(!r.ok);
    assert_eq!(
        r.error.as_deref(),
        Some("mount namespace differs; not deleting"),
        "{r:?}"
    );
    assert!(env.sys("/opt/inky/sub/b").exists());
    // The step itself exited 97.
    let out = std::process::Command::new("/bin/sh")
        .arg("-c")
        .arg(std::ffi::OsStr::from_bytes(&calls[0]))
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap();
    let markers = crate::privilege::batch::parse_markers(&out.stdout, 1);
    assert_eq!(markers[0].status, Some(97));
    assert!(env.sys("/opt/inky/sub/b").exists());
}

#[test]
fn system_tree_delete_refused_when_namespace_unreadable() {
    let (_t, mut env) = system_tree_fixture();
    env.set_mount_ns(None);
    let plan = plan_of(vec![tree_delete(&env, "/opt/inky")]);
    let (report, calls) = run_tree(&env, &plan);
    assert_eq!(
        report.results[0].error.as_deref(),
        Some("the mount namespace could not be read")
    );
    assert!(calls.is_empty());
    assert!(env.sys("/opt/inky/a").exists());
}

#[test]
fn system_tree_delete_refused_when_mount_below() {
    let (t, env) = system_tree_fixture();
    let info = [
        mountinfo_line(Path::new("/")),
        mountinfo_line(&t.dir.path().join("opt/inky/data")),
    ]
    .concat();
    t.write("proc/self/mountinfo", info.as_bytes());
    let plan = plan_of(vec![
        tree_delete(&env, "/opt/inky"),
        tree_delete(&env, "/opt/blot"),
    ]);
    let (report, calls) = run_tree(&env, &plan);
    assert_eq!(
        report.results[0].error.as_deref(),
        Some("a mount point is below it")
    );
    assert!(report.results[1].ok, "{:?}", report.results[1]);
    assert_eq!(step_count(&calls[0]), 1);
    assert!(!String::from_utf8_lossy(&calls[0]).contains("inky"));
    assert!(env.sys("/opt/inky/a").exists());
    assert!(!env.sys("/opt/blot").exists());
}

#[test]
fn system_tree_delete_refused_when_mountinfo_unreadable() {
    let (t, env) = system_tree_fixture();
    t.write("proc/self/mountinfo", b"");
    let plan = plan_of(vec![tree_delete(&env, "/opt/inky")]);
    let (report, calls) = run_tree(&env, &plan);
    assert_eq!(
        report.results[0].error.as_deref(),
        Some("the mount table could not be read")
    );
    assert!(calls.is_empty());
    assert!(env.sys("/opt/inky/a").exists());
}

#[test]
fn system_tree_delete_refused_when_owned_since_scan() {
    let (t, env) = system_tree_fixture();
    let plan = plan_of(vec![
        tree_delete(&env, "/opt/inky"),
        tree_delete(&env, "/opt/blot"),
    ]);
    t.pacman_pkg("inky", "2-1", Some(1), &["opt/inky/new"]);
    let (report, _) = run_tree(&env, &plan);
    assert_eq!(
        report.results[0].error.as_deref(),
        Some("a package owns files in it now")
    );
    assert!(report.results[1].ok, "{:?}", report.results[1]);
    assert!(env.sys("/opt/inky/a").exists());
}

#[test]
fn system_tree_delete_refused_while_pacman_locked() {
    let (t, env) = system_tree_fixture();
    t.write("var/lib/pacman/db.lck", b"");
    let plan = plan_of(vec![tree_delete(&env, "/opt/inky")]);
    let (report, calls) = run_tree(&env, &plan);
    assert_eq!(
        report.results[0].error.as_deref(),
        Some("pacman is busy (database lock present)")
    );
    assert!(calls.is_empty());
    assert!(env.sys("/opt/inky/a").exists());
}

#[test]
fn system_tree_delete_refused_while_a_package_manager_runs() {
    for name in ["dpkg", "apt-get", "dnf5", "rpm", "unattended-upgr"] {
        let (t, env) = system_tree_fixture();
        t.write("proc/4242/comm", format!("{name}\n").as_bytes());
        let plan = plan_of(vec![tree_delete(&env, "/opt/inky")]);
        let (report, calls) = run_tree(&env, &plan);
        assert_eq!(
            report.results[0].error.as_deref(),
            Some(TREE_PM_RUNNING),
            "{name}"
        );
        assert!(calls.is_empty());
        assert!(env.sys("/opt/inky/a").exists());
    }
    // An unrelated process does not block.
    let (t, env) = system_tree_fixture();
    t.write("proc/4242/comm", b"bash\n");
    let plan = plan_of(vec![tree_delete(&env, "/opt/inky")]);
    let (report, _) = run_tree(&env, &plan);
    assert_ne!(report.results[0].error.as_deref(), Some(TREE_PM_RUNNING));
}

#[test]
fn system_tree_delete_refused_without_package_database() {
    let (t, env) = system_tree_fixture();
    fs::remove_dir_all(t.dir.path().join("var/lib/pacman/local")).unwrap();
    let plan = plan_of(vec![tree_delete(&env, "/opt/inky")]);
    let (report, calls) = run_tree(&env, &plan);
    assert_eq!(
        report.results[0].error.as_deref(),
        Some("the package database could not be read")
    );
    assert!(calls.is_empty());
    assert!(env.sys("/opt/inky/a").exists());

    // An unreadable entry fails closed the same way.
    let (t, env) = system_tree_fixture();
    t.write("var/lib/pacman/local/bad-1-1/files/x", b"");
    let plan = plan_of(vec![tree_delete(&env, "/opt/inky")]);
    let (report, _) = run_tree(&env, &plan);
    assert_eq!(
        report.results[0].error.as_deref(),
        Some("the package database could not be read")
    );
    assert!(env.sys("/opt/inky/a").exists());
}

#[test]
fn system_tree_history_and_restore() {
    let (_t, env) = system_tree_fixture();
    let plan = plan_of(vec![tree_delete(&env, "/opt/inky")]);
    let (report, _) = run_tree(&env, &plan);
    assert!(report.results[0].ok, "{:?}", report.results[0]);
    let raw = fs::read_to_string(env.state_home.join("crumbs/history.jsonl")).unwrap();
    assert!(raw.contains("\"scope\":\"system-tree\""), "{raw}");
    let (hist, _) = read_history(&env).unwrap();
    assert_eq!(hist[0].scope, Some(Scope::SystemTree));
    let cfg = Config::default();
    let id = report.results[0].history_id.clone();
    let out = crate::restore::restore(&env, &cfg, &[id], false).unwrap();
    assert_eq!(out.len(), 1);
    assert!(!out[0].ok);
    assert_eq!(
        out[0].error.as_deref(),
        Some(crate::restore::MSG_SYSTEM_TREE)
    );
    let out =
        crate::restore::restore(&env, &cfg, std::slice::from_ref(&report.batch), false).unwrap();
    assert_eq!(out.len(), 1);
    assert!(out[0].ok, "{out:?}");
    assert_eq!(
        out[0].error.as_deref(),
        Some(crate::restore::NOTHING_RESTORABLE)
    );
}

#[test]
fn home_delete_history_has_no_scope() {
    let t = FixtureTree::new();
    t.write("home/user/.cache/foo/a", b"a");
    let env = t.env();
    let plan = plan_of(vec![planned(&env, delete("/home/user/.cache/foo"))]);
    let report = execute(&env, &Config::default(), &plan);
    assert!(report.results[0].ok, "{:?}", report.results[0]);
    let raw = fs::read_to_string(env.state_home.join("crumbs/history.jsonl")).unwrap();
    assert!(!raw.contains("\"scope\""), "{raw}");
}

#[test]
fn system_tree_step_refuses_when_lock_appears_in_the_batch() {
    let (t, env) = system_tree_fixture();
    // The lock appears after the user-side re-check: the step's `sh`
    // creates it just before running the guard.
    let lock = env.sys("/var/lib/pacman/db.lck");
    let sh = format!("#!/bin/sh\n: > '{}'\nexec /bin/sh \"$@\"\n", lock.display());
    t.write_exec("usr/bin/sh", sh.as_bytes());
    let plan = plan_of(vec![tree_delete(&env, "/opt/inky")]);
    let (report, calls) = run_tree(&env, &plan);
    let r = &report.results[0];
    assert!(!r.ok);
    assert_eq!(
        r.error.as_deref(),
        Some("pacman is busy (database lock present); not deleting"),
        "{r:?}"
    );
    assert!(env.sys("/opt/inky/sub/b").exists());
    // The step itself exited 97.
    let out = std::process::Command::new("/bin/sh")
        .arg("-c")
        .arg(std::ffi::OsStr::from_bytes(&calls[0]))
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap();
    let markers = crate::privilege::batch::parse_markers(&out.stdout, 1);
    assert_eq!(markers[0].status, Some(97));
    assert!(env.sys("/opt/inky/sub/b").exists());
}

#[test]
fn system_tree_delete_refused_when_tmpfiles_declares_it_now() {
    let (t, env) = system_tree_fixture();
    let plan = plan_of(vec![
        tree_delete(&env, "/opt/inky"),
        tree_delete(&env, "/opt/blot"),
    ]);
    t.write("usr/lib/tmpfiles.d/inky.conf", b"d /opt/inky/run 0755\n");
    let (report, calls) = run_tree(&env, &plan);
    assert_eq!(
        report.results[0].error.as_deref(),
        Some("a package owns files in it now")
    );
    assert!(report.results[1].ok, "{:?}", report.results[1]);
    assert_eq!(step_count(&calls[0]), 1);
    assert!(env.sys("/opt/inky/a").exists());
    assert!(!env.sys("/opt/blot").exists());

    // Unreadable `tmpfiles.d` fails closed.
    let (t, env) = system_tree_fixture();
    t.write("etc/tmpfiles.d", b"not a directory");
    let plan = plan_of(vec![tree_delete(&env, "/opt/inky")]);
    let (report, calls) = run_tree(&env, &plan);
    assert_eq!(
        report.results[0].error.as_deref(),
        Some("the package database could not be read")
    );
    assert!(calls.is_empty());
    assert!(env.sys("/opt/inky/a").exists());
}

#[test]
fn system_tree_rechecks_read_pacman_once_per_batch() {
    let (_t, env) = system_tree_fixture();
    let plan = plan_of(vec![
        tree_delete(&env, "/opt/inky"),
        tree_delete(&env, "/opt/blot"),
    ]);
    super::PACMAN_LOADS.with(|n| n.set(0));
    let (report, _) = run_tree(&env, &plan);
    assert!(report.results.iter().all(|r| r.ok), "{report:?}");
    assert_eq!(super::PACMAN_LOADS.with(std::cell::Cell::get), 1);
}

#[test]
fn system_tree_delete_refused_without_trusted_readlink() {
    let (t, env) = system_tree_fixture();
    fs::remove_file(t.dir.path().join("usr/bin/readlink")).unwrap();
    let plan = plan_of(vec![tree_delete(&env, "/opt/inky")]);
    let (report, calls) = run_tree(&env, &plan);
    assert_eq!(
        report.results[0].error.as_deref(),
        Some("the mount namespace could not be read")
    );
    assert!(calls.is_empty());
    assert!(env.sys("/opt/inky/a").exists());
}

#[test]
fn failed_system_tree_delete_restore_says_system_tree() {
    let (t, env) = system_tree_fixture();
    t.write("var/lib/pacman/db.lck", b"");
    let plan = plan_of(vec![tree_delete(&env, "/opt/inky")]);
    let (report, _) = run_tree(&env, &plan);
    assert!(!report.results[0].ok);
    let id = report.results[0].history_id.clone();
    let out = crate::restore::restore(&env, &Config::default(), &[id], false).unwrap();
    assert_eq!(out.len(), 1);
    assert!(!out[0].ok);
    assert_eq!(
        out[0].error.as_deref(),
        Some(crate::restore::MSG_SYSTEM_TREE)
    );
}

#[test]
fn only_root_deletes_record_a_scope() {
    let t = FixtureTree::new();
    t.write("home/user/.local/share/Trash/files/x", b"x");
    let env = t.env();
    let mut pa = planned(&env, delete("/home/user/.local/share/Trash/files/x"));
    pa.scope = Scope::HomeTrash;
    let plan = plan_of(vec![pa]);
    let report = execute(&env, &Config::default(), &plan);
    let raw = fs::read_to_string(env.state_home.join("crumbs/history.jsonl")).unwrap();
    assert!(!raw.contains("\"scope\""), "{report:?} {raw}");
}

/// A finding for an existing in-system path, as the scan makes it.
fn scanned(
    env: &Env,
    path: &str,
    kind: crate::leftovers::FindingKind,
    scope: Scope,
) -> crate::leftovers::Finding {
    let p = PathBuf::from(path);
    crate::leftovers::Finding {
        size: disk_usage(env, &p),
        fingerprint: fingerprint(env, &p).unwrap(),
        path: p,
        kind,
        confidence: crate::leftovers::Confidence::Medium,
        shared: false,
        reason: "test".into(),
        preselected: scope == Scope::Home,
        scope,
        mime_lines: Vec::new(),
    }
}

#[test]
fn unmanaged_uninstall_with_picked_system_leftover_runs_end_to_end() {
    use crate::leftovers::FindingKind;
    use crate::plan::{PlanOptions, build_plan_unmanaged};
    let (t, env) = system_tree_fixture();
    fs::create_dir_all(t.dir.path().join("etc/inky")).unwrap();
    t.write("etc/inky/inky.conf", b"x");
    fs::set_permissions(t.dir.path().join("etc"), fs::Permissions::from_mode(0o755)).unwrap();
    t.write("home/user/.cache/inky/blob", b"b");
    t.symlink("home/user/.local/bin/inky", "/opt/inky/a");
    let mut app = crate::model::App::new(SourceKind::Unmanaged, "unmanaged:inky.desktop");
    app.program = Some("/opt/inky/a".into());
    let target = crate::removal::UnmanagedTarget {
        path: "/opt/inky".into(),
        scope: Scope::SystemTree,
        fingerprint: fingerprint(&env, Path::new("/opt/inky")).unwrap(),
        size: disk_usage(&env, Path::new("/opt/inky")),
        left_link: None,
        left_link_protected: false,
        home_link: Some((
            "/home/user/.local/bin/inky".into(),
            fingerprint(&env, Path::new("/home/user/.local/bin/inky")).unwrap(),
        )),
    };
    let findings = [
        scanned(
            &env,
            "/home/user/.cache/inky",
            FindingKind::Cache,
            Scope::Home,
        ),
        scanned(
            &env,
            "/etc/inky",
            FindingKind::SystemConfig,
            Scope::SystemTree,
        ),
    ];
    let plan = build_plan_unmanaged(
        &env,
        Some(&app),
        Some(&target),
        &findings,
        &PlanOptions::default(),
    )
    .unwrap();
    let deps: Vec<Option<usize>> = plan.actions.iter().map(|a| a.depends_on).collect();
    // Home actions wait for the target; the picked system leftover waits
    // for nothing.
    assert_eq!(deps, [None, Some(0), Some(0), None]);
    let (report, calls) = run_tree(&env, &plan);
    assert_eq!(calls.len(), 1, "one batch");
    assert_eq!(step_count(&calls[0]), 2);
    for r in &report.results {
        assert!(r.ok, "{r:?}");
    }
    assert!(!env.sys("/opt/inky").exists());
    assert!(!env.sys("/etc/inky").exists());
    assert!(!env.sys("/home/user/.cache/inky").exists());
    assert!(fs::symlink_metadata(env.sys("/home/user/.local/bin/inky")).is_err());
}

#[test]
fn system_tree_step_refuses_dangling_symlink_lock() {
    let (t, env) = system_tree_fixture();
    // A dangling symlink appears at the lock inside the batch.
    let lock = env.sys("/var/lib/pacman/db.lck");
    let sh = format!(
        "#!/bin/sh\nln -s /nonexistent-crumbs-target '{}'\nexec /bin/sh \"$@\"\n",
        lock.display()
    );
    t.write_exec("usr/bin/sh", sh.as_bytes());
    let plan = plan_of(vec![tree_delete(&env, "/opt/inky")]);
    let (report, _) = run_tree(&env, &plan);
    let r = &report.results[0];
    assert!(!r.ok);
    assert_eq!(
        r.error.as_deref(),
        Some("pacman is busy (database lock present); not deleting"),
        "{r:?}"
    );
    assert!(
        fs::symlink_metadata(&lock)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert!(!lock.exists(), "dangling");
    assert!(env.sys("/opt/inky/sub/b").exists());
}

/// A privileged runner that removes `remove` (real paths) as the batch
/// would, then hands back canned markers.
struct RemovingRunner {
    remove: Vec<PathBuf>,
    out: Vec<u8>,
}

impl PrivilegedRunner for RemovingRunner {
    fn run(&self, _: &Escalation, _: &[u8]) -> Result<Vec<u8>, crate::error::CoreError> {
        for p in &self.remove {
            let _ = std::fs::remove_file(p);
        }
        Ok(self.out.clone())
    }
}

/// A command runner that removes `remove` and succeeds.
struct RemovingCommand(Vec<PathBuf>);

impl CommandRunner for RemovingCommand {
    fn run(
        &self,
        _: &Path,
        _: &[&str],
    ) -> Result<crate::command::CmdOutput, crate::error::CoreError> {
        for p in &self.0 {
            let _ = std::fs::remove_file(p);
        }
        Ok(crate::command::CmdOutput {
            status: 0,
            stdout: String::new(),
            stderr: String::new(),
            timed_out: false,
        })
    }
}

#[test]
fn measured_dirs_of_known_cleanup_commands() {
    let t = FixtureTree::new();
    let env = t.env();
    let dirs = |program: &str, args: &[&str]| {
        measured_dirs(
            &env,
            &Action::PrivilegedCommand {
                program: program.into(),
                args: args.iter().map(|a| (*a).to_string()).collect(),
            },
        )
    };
    let p = |s: &str| vec![PathBuf::from(s)];
    assert_eq!(dirs("paccache", &["-rk2"]), p("/var/cache/pacman/pkg"));
    assert_eq!(
        dirs("pacman", &["-Sc", "--noconfirm"]),
        p("/var/cache/pacman/pkg")
    );
    assert_eq!(
        dirs("journalctl", &["--vacuum-size=500M"]),
        p("/var/log/journal")
    );
    assert_eq!(dirs("apt-get", &["clean"]), p("/var/cache/apt/archives"));
    assert_eq!(
        dirs("dnf", &["clean", "packages"]),
        vec![
            PathBuf::from("/var/cache/dnf"),
            PathBuf::from("/var/cache/libdnf5")
        ]
    );
    assert_eq!(
        dirs(
            "systemd-tmpfiles",
            &["--clean", "--prefix=/tmp", "--prefix=/var/tmp"]
        ),
        vec![PathBuf::from("/tmp"), PathBuf::from("/var/tmp")]
    );
    assert_eq!(
        dirs(
            "flatpak",
            &[
                "uninstall",
                "--system",
                "--unused",
                "-y",
                "--noninteractive"
            ]
        ),
        p("/var/lib/flatpak")
    );
    let user = measured_dirs(
        &env,
        &Action::UserCommand {
            program: "flatpak".into(),
            args: ["uninstall", "--user", "--unused", "--noninteractive", "-y"]
                .map(String::from)
                .to_vec(),
        },
    );
    assert_eq!(user, p("/home/user/.local/share/flatpak"));
    // Anything else is not measured: an app's uninstall, an unknown tool.
    assert!(dirs("flatpak", &["uninstall", "--system", "-y", "org.app"]).is_empty());
    assert!(dirs("snap", &["remove", "--purge", "x"]).is_empty());
}

#[test]
fn privileged_cleanup_reports_measured_space() {
    let t = FixtureTree::new();
    let log = t.dir.path().join("calls.log");
    logging_program(&t, "paccache", &log, "", 0);
    let old = t.write("var/cache/pacman/pkg/a-1.pkg.tar.zst", &[1u8; 20_000]);
    t.write("var/cache/pacman/pkg/a-2.pkg.tar.zst", &[1u8; 20_000]);
    let env = t.env();
    let gone =
        crate::size::disk_usage(&env, Path::new("/var/cache/pacman/pkg/a-1.pkg.tar.zst")).bytes;
    let mut pa = priv_cmd("paccache", &["-rk1"]);
    pa.size = 999_999; // the scan's estimate
    let report = execute_full(
        &env,
        &Config::default(),
        &plan_of(vec![pa]),
        &FakeRunner::new(),
        &RemovingRunner {
            remove: vec![old],
            out: b"@@crumbs 0 0\n".to_vec(),
        },
        Some(&esc()),
        &no_post_check,
    );
    assert!(report.results[0].ok, "{:?}", report.results[0]);
    assert_eq!(report.results[0].freed, Some(gone));
    assert_eq!(report.freed, gone);
}

#[test]
fn user_cleanup_reports_measured_space() {
    let t = FixtureTree::new();
    logging_program(&t, "flatpak", &t.dir.path().join("l.log"), "", 0);
    let rt = t.write(
        "home/user/.local/share/flatpak/runtime/x/files/lib.so",
        &[1u8; 12_000],
    );
    t.write(
        "home/user/.local/share/flatpak/app/y/files/bin",
        &[1u8; 4_000],
    );
    let env = t.env();
    let gone = crate::size::disk_usage(
        &env,
        Path::new("/home/user/.local/share/flatpak/runtime/x/files/lib.so"),
    )
    .bytes;
    let plan = command_plan(
        "flatpak",
        &["uninstall", "--user", "--unused", "--noninteractive", "-y"],
    );
    let report = execute_with(&env, &Config::default(), &plan, &RemovingCommand(vec![rt]));
    assert!(report.results[0].ok, "{:?}", report.results[0]);
    assert_eq!(report.results[0].freed, Some(gone));
    assert_eq!(report.freed, gone);
}

#[test]
fn unmeasurable_cleanup_keeps_the_estimate() {
    // The folder does not exist: nothing to measure, so the scan's
    // estimate stands and the result carries no measurement.
    let t = FixtureTree::new();
    let log = t.dir.path().join("calls.log");
    logging_program(&t, "journalctl", &log, "", 0);
    let env = t.env();
    let mut pa = priv_cmd("journalctl", &["--vacuum-size=500M"]);
    pa.size = 4096;
    let report = execute_full(
        &env,
        &Config::default(),
        &plan_of(vec![pa]),
        &FakeRunner::new(),
        &CannedRunner(b"@@crumbs 0 0\n".to_vec()),
        Some(&esc()),
        &no_post_check,
    );
    assert!(report.results[0].ok, "{:?}", report.results[0]);
    assert_eq!(report.results[0].freed, None);
    assert_eq!(report.freed, 4096);
}

#[test]
fn measurement_tolerates_unreadable_parts_that_stay() {
    // pacman 7 leaves root-only download-* folders in the cache: they are
    // unreadable before and after, so what was read is still compared.
    use std::os::unix::fs::PermissionsExt;
    let t = FixtureTree::new();
    logging_program(&t, "paccache", &t.dir.path().join("l.log"), "", 0);
    let old = t.write("var/cache/pacman/pkg/a-1.pkg.tar.zst", &[1u8; 20_000]);
    t.write("var/cache/pacman/pkg/a-2.pkg.tar.zst", &[1u8; 20_000]);
    let locked = t.mkdir("var/cache/pacman/pkg/download-Xy12");
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
    let env = t.env();
    let gone =
        crate::size::disk_usage(&env, Path::new("/var/cache/pacman/pkg/a-1.pkg.tar.zst")).bytes;
    let report = execute_full(
        &env,
        &Config::default(),
        &plan_of(vec![priv_cmd("paccache", &["-rk1"])]),
        &FakeRunner::new(),
        &RemovingRunner {
            remove: vec![old],
            out: b"@@crumbs 0 0\n".to_vec(),
        },
        Some(&esc()),
        &no_post_check,
    );
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(report.results[0].ok, "{:?}", report.results[0]);
    assert_eq!(report.results[0].freed, Some(gone));
}

#[test]
fn measurement_counts_a_folder_missing_both_times_as_empty() {
    // dnf: only one of /var/cache/dnf and /var/cache/libdnf5 exists.
    let t = FixtureTree::new();
    logging_program(&t, "dnf", &t.dir.path().join("l.log"), "", 0);
    let old = t.write("var/cache/libdnf5/repo/packages/a.rpm", &[1u8; 12_000]);
    let env = t.env();
    let gone =
        crate::size::disk_usage(&env, Path::new("/var/cache/libdnf5/repo/packages/a.rpm")).bytes;
    let report = execute_full(
        &env,
        &Config::default(),
        &plan_of(vec![priv_cmd("dnf", &["clean", "packages"])]),
        &FakeRunner::new(),
        &RemovingRunner {
            remove: vec![old],
            out: b"@@crumbs 0 0\n".to_vec(),
        },
        Some(&esc()),
        &no_post_check,
    );
    assert!(report.results[0].ok, "{:?}", report.results[0]);
    assert_eq!(report.results[0].freed, Some(gone));
}
