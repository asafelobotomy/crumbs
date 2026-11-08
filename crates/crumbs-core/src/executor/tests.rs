use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use super::*;
use crate::history::{HistoryAction, read_history};
use crate::model::SourceKind;
use crate::plan::PlannedAction;
use crate::safety::fingerprint;
use crate::size::disk_usage;
use crate::testing::FixtureTree;

/// Like `build_plan`: edits free nothing, removals free the disk usage.
fn planned(env: &Env, action: Action) -> PlannedAction {
    let p = action.path().to_path_buf();
    let size = match action {
        Action::EditFile { .. } => 0,
        _ => disk_usage(env, &p).bytes,
    };
    PlannedAction {
        size,
        fingerprint: fingerprint(env, &p).unwrap(),
        reason: "test".into(),
        action,
    }
}

fn plan_of(actions: Vec<PlannedAction>) -> Plan {
    Plan {
        app_id: Some("pacman:foo".into()),
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
        assert_eq!(h.path, plan.actions[i].action.path());
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
        fs::read(env.sys("/home/user/elsewhere.list")).unwrap(),
        MIME
    );
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
