use super::*;
use crate::config::Config;
use crate::executor::{ExecReport, execute};
use crate::history::{self, HistoryAction, HistoryEntry, read_history};
use crate::plan::{Action, Plan, PlannedAction};
use crate::safety::fingerprint;
use crate::testing::FixtureTree;
use std::fs;
use std::path::Path;

fn run(env: &Env, actions: Vec<Action>) -> ExecReport {
    let actions: Vec<PlannedAction> = actions
        .into_iter()
        .map(|action| PlannedAction {
            size: 1,
            fingerprint: fingerprint(env, action.path().unwrap()).unwrap(),
            scope: crate::safety::Scope::Home,
            depends_on: None,
            depends_on_plan: None,
            size_known: true,
            reason: "test".into(),
            action,
        })
        .collect();
    let plan = Plan {
        app_id: Some("pacman:foo".into()),
        kind: None,
        total_size: actions.len() as u64,
        actions,
        notes: Vec::new(),
    };
    let report = execute(env, &Config::default(), &plan);
    assert!(report.results.iter().all(|r| r.ok), "{report:?}");
    report
}

fn trash(p: &str) -> Action {
    Action::Trash { path: p.into() }
}

fn sel(s: &str) -> Vec<String> {
    vec![s.to_string()]
}

fn restore_entries(env: &Env) -> Vec<HistoryEntry> {
    read_history(env)
        .unwrap()
        .0
        .into_iter()
        .filter(|e| e.action == HistoryAction::Restore)
        .collect()
}

#[test]
fn restore_by_batch() {
    let t = FixtureTree::new();
    t.write("home/user/.config/a/x", b"A");
    t.write("home/user/.cache/b/y", b"B");
    let env = t.env();
    let rep = run(
        &env,
        vec![trash("/home/user/.config/a"), trash("/home/user/.cache/b")],
    );
    let out = restore(&env, &Config::default(), &sel(&rep.batch), false).unwrap();
    assert_eq!(out.len(), 2);
    assert!(out.iter().all(|o| o.ok), "{out:?}");
    assert_eq!(out[0].entry_id, rep.results[0].history_id);
    assert_eq!(out[0].path, Path::new("/home/user/.config/a"));
    assert_eq!(fs::read(env.sys("/home/user/.config/a/x")).unwrap(), b"A");
    assert_eq!(fs::read(env.sys("/home/user/.cache/b/y")).unwrap(), b"B");
    let logged = restore_entries(&env);
    assert_eq!(logged.len(), 2);
    assert!(logged.iter().all(|e| e.ok));
    assert_eq!(
        logged[0].restores.as_deref(),
        Some(rep.results[0].history_id.as_str())
    );
    assert_ne!(logged[0].batch, rep.batch);
}

/// Each Restore entry is appended right after its action, on its own, so
/// a failing append costs only that entry.
#[test]
fn restore_appends_per_entry() {
    use std::cell::RefCell;
    let t = FixtureTree::new();
    t.write("home/user/.config/a/x", b"A");
    t.write("home/user/.config/b/y", b"B");
    t.write("home/user/.config/c/z", b"C");
    let env = t.env();
    let rep = run(
        &env,
        vec![
            trash("/home/user/.config/a"),
            trash("/home/user/.config/b"),
            trash("/home/user/.config/c"),
        ],
    );
    let calls: RefCell<Vec<usize>> = RefCell::new(Vec::new());
    let append = |env: &Env, entries: &[HistoryEntry]| {
        let mut calls = calls.borrow_mut();
        calls.push(entries.len());
        if calls.len() == 2 {
            return Err(CoreError::Parse {
                what: "history entry".into(),
                message: "boom".into(),
            });
        }
        history::append(env, entries)
    };
    let out = restore_with(&env, &Config::default(), &sel(&rep.batch), false, &append).unwrap();
    assert_eq!(*calls.borrow(), [1, 1, 1]);
    assert!(out.iter().all(|o| o.ok), "{out:?}");
    assert_eq!(out[0].error, None, "{out:?}");
    assert!(
        out[1]
            .error
            .as_deref()
            .unwrap()
            .ends_with("; history not written"),
        "{out:?}"
    );
    assert_eq!(out[2].error, None, "{out:?}");
    // The restores themselves all happened, and the other two are logged.
    for d in ["a/x", "b/y", "c/z"] {
        assert!(env.sys(&format!("/home/user/.config/{d}")).exists());
    }
    let logged = restore_entries(&env);
    assert_eq!(logged.len(), 2);
    assert_eq!(
        logged[0].restores.as_deref(),
        Some(rep.results[0].history_id.as_str())
    );
    assert_eq!(
        logged[1].restores.as_deref(),
        Some(rep.results[2].history_id.as_str())
    );
}

#[test]
fn restore_by_path_newest() {
    let t = FixtureTree::new();
    t.write("home/user/.config/foo/v", b"old");
    let env = t.env();
    let first = run(&env, vec![trash("/home/user/.config/foo")]);
    t.write("home/user/.config/foo/v", b"new");
    let second = run(&env, vec![trash("/home/user/.config/foo")]);
    assert_ne!(first.batch, second.batch);
    let out = restore(
        &env,
        &Config::default(),
        &sel("/home/user/.config/foo"),
        false,
    )
    .unwrap();
    assert_eq!(out.len(), 1);
    assert!(out[0].ok, "{out:?}");
    assert_eq!(out[0].entry_id, second.results[0].history_id);
    assert_eq!(
        fs::read(env.sys("/home/user/.config/foo/v")).unwrap(),
        b"new"
    );
    let (hist, _) = read_history(&env).unwrap();
    let old = hist[0].trashed.as_ref().unwrap();
    assert!(env.sys_path(&old.files_path).exists());
}

/// Review Focus #4.
#[test]
fn restore_partial_when_occupied() {
    let t = FixtureTree::new();
    t.write("home/user/.config/a/x", b"A");
    t.write("home/user/.config/b/y", b"B");
    let env = t.env();
    let rep = run(
        &env,
        vec![trash("/home/user/.config/a"), trash("/home/user/.config/b")],
    );
    t.write("home/user/.config/a/x", b"recreated");
    let out = restore(&env, &Config::default(), &sel(&rep.batch), false).unwrap();
    assert!(!out[0].ok);
    assert!(
        out[0].error.as_deref().unwrap().contains("exists"),
        "{out:?}"
    );
    assert!(out[1].ok, "{out:?}");
    assert_eq!(
        fs::read(env.sys("/home/user/.config/a/x")).unwrap(),
        b"recreated"
    );
    assert_eq!(fs::read(env.sys("/home/user/.config/b/y")).unwrap(), b"B");
    let (hist, _) = read_history(&env).unwrap();
    let a = hist[0].trashed.as_ref().unwrap();
    assert_eq!(
        fs::read(env.sys_path(&a.files_path).join("x")).unwrap(),
        b"A"
    );
    assert!(env.sys_path(&a.info_path).exists());
    let logged: Vec<bool> = restore_entries(&env).iter().map(|e| e.ok).collect();
    assert_eq!(logged, [false, true]);
}

#[test]
fn restore_twice_is_noop_error() {
    let t = FixtureTree::new();
    t.write("home/user/.config/a/x", b"A");
    let env = t.env();
    let rep = run(&env, vec![trash("/home/user/.config/a")]);
    let id = rep.results[0].history_id.clone();
    assert!(restore(&env, &Config::default(), &sel(&id), false).unwrap()[0].ok);
    let again = restore(&env, &Config::default(), &sel(&id), false).unwrap();
    assert_eq!(again.len(), 1);
    assert!(!again[0].ok);
    assert_eq!(again[0].error.as_deref(), Some("already restored"));
    assert_eq!(fs::read(env.sys("/home/user/.config/a/x")).unwrap(), b"A");
}

fn edit_mime(env: &Env, t: &FixtureTree, original: &[u8]) -> ExecReport {
    t.write("home/user/.config/mimeapps.list", original);
    run(
        env,
        vec![Action::EditFile {
            path: "/home/user/.config/mimeapps.list".into(),
            remove_lines: vec!["text/plain=foo.desktop".into()],
        }],
    )
}

#[test]
fn restore_edit_file_keeps_newer_content() {
    let t = FixtureTree::new();
    let env = t.env();
    let original = b"text/plain=foo.desktop\na=b\n";
    let rep = edit_mime(&env, &t, original);
    let file = env.sys("/home/user/.config/mimeapps.list");
    fs::write(&file, b"a=b\nnewer=de.desktop\n").unwrap();
    let out = restore(
        &env,
        &Config::default(),
        &sel(&rep.results[0].history_id),
        false,
    )
    .unwrap();
    assert!(out[0].ok, "{out:?}");
    assert_eq!(fs::read(&file).unwrap(), original);
    let logged = restore_entries(&env);
    let backup = logged[0].backup.clone().unwrap();
    assert_eq!(
        backup,
        Path::new(&format!(
            "/home/user/.config/mimeapps.list.crumbs-bak-{}",
            logged[0].batch
        ))
    );
    assert_eq!(
        fs::read(env.sys_path(&backup)).unwrap(),
        b"a=b\nnewer=de.desktop\n"
    );
}

#[test]
fn restore_edit_file_refuses_symlink() {
    let t = FixtureTree::new();
    let env = t.env();
    let rep = edit_mime(&env, &t, b"text/plain=foo.desktop\na=b\n");
    let file = env.sys("/home/user/.config/mimeapps.list");
    fs::remove_file(&file).unwrap();
    t.write("home/user/other.list", b"other");
    t.symlink("home/user/.config/mimeapps.list", "../other.list");
    let out = restore(
        &env,
        &Config::default(),
        &sel(&rep.results[0].history_id),
        false,
    )
    .unwrap();
    assert!(!out[0].ok);
    assert_eq!(
        out[0].error.as_deref(),
        Some("refusing: ~/.config/mimeapps.list is not a regular file")
    );
    assert!(
        fs::symlink_metadata(&file)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(
        fs::read(env.sys("/home/user/other.list")).unwrap(),
        b"other"
    );
}

#[test]
fn restore_respects_user_protect() {
    let t = FixtureTree::new();
    t.write("home/user/.config/a/x", b"A");
    let env = t.env();
    let rep = run(&env, vec![trash("/home/user/.config/a")]);
    let cfg = Config {
        protect: vec!["/home/user/.config/a".into()],
        ..Config::default()
    };
    let out = restore(&env, &cfg, &sel(&rep.batch), false).unwrap();
    assert!(!out[0].ok);
    assert_eq!(out[0].error.as_deref(), Some(INVALID_ENTRY));
    assert!(!env.sys("/home/user/.config/a").exists());
}

#[test]
fn restore_edit_file_from_backup() {
    let t = FixtureTree::new();
    let original = b"[Default Applications]\ntext/plain=foo.desktop\na=b\n";
    t.write("home/user/.config/mimeapps.list", original);
    let env = t.env();
    let rep = run(
        &env,
        vec![Action::EditFile {
            path: "/home/user/.config/mimeapps.list".into(),
            remove_lines: vec!["text/plain=foo.desktop".into()],
        }],
    );
    let file = env.sys("/home/user/.config/mimeapps.list");
    assert_ne!(fs::read(&file).unwrap(), original);
    let out = restore(
        &env,
        &Config::default(),
        &sel(&rep.results[0].history_id),
        false,
    )
    .unwrap();
    assert!(out[0].ok, "{out:?}");
    assert_eq!(fs::read(&file).unwrap(), original);
}

#[test]
fn restore_unknown_selector() {
    let t = FixtureTree::new();
    let env = t.env();
    let out = restore(&env, &Config::default(), &sel("12345-0"), false).unwrap();
    assert_eq!(out.len(), 1);
    assert!(!out[0].ok);
    assert_eq!(out[0].error.as_deref(), Some("no matching history entry"));
    assert!(read_history(&env).unwrap().0.is_empty());
}

#[test]
fn restore_dry_run_changes_nothing() {
    let t = FixtureTree::new();
    t.write("home/user/.config/a/x", b"A");
    let env = t.env();
    let rep = run(&env, vec![trash("/home/user/.config/a")]);
    let out = restore(&env, &Config::default(), &sel(&rep.batch), true).unwrap();
    assert!(out[0].ok, "{out:?}");
    assert!(!env.sys("/home/user/.config/a").exists());
    assert!(restore_entries(&env).is_empty());
}

#[test]
fn restore_rejects_tampered_entry() {
    let t = FixtureTree::new();
    t.write("home/user/.config/a/x", b"A");
    t.write("home/user/Documents/precious", b"P");
    let env = t.env();
    let rep = run(&env, vec![trash("/home/user/.config/a")]);
    let (hist, _) = read_history(&env).unwrap();
    let good = hist[0].clone();
    let mut bad = Vec::new();
    // files_path pointing at a file outside the trash.
    let mut e = good.clone();
    e.id = "1-0".into();
    e.batch = "1".into();
    if let Some(tr) = e.trashed.as_mut() {
        tr.files_path = "/home/user/Documents/precious".into();
    }
    bad.push(e);
    // Original outside home.
    let mut e = good.clone();
    e.id = "1-1".into();
    e.batch = "1".into();
    e.path = "/etc/passwd".into();
    if let Some(tr) = e.trashed.as_mut() {
        tr.original = "/etc/passwd".into();
    }
    bad.push(e);
    // Non-canonical original.
    let mut e = good.clone();
    e.id = "1-2".into();
    e.batch = "1".into();
    e.path = "/home/user/.config/../.config/a".into();
    if let Some(tr) = e.trashed.as_mut() {
        tr.original = e.path.clone();
    }
    bad.push(e);
    history::append(&env, &bad).unwrap();
    let out = restore(&env, &Config::default(), &sel("1"), false).unwrap();
    assert_eq!(out.len(), 3);
    for o in &out {
        assert!(!o.ok);
        assert_eq!(o.error.as_deref(), Some("history entry failed validation"));
    }
    assert_eq!(
        fs::read(env.sys("/home/user/Documents/precious")).unwrap(),
        b"P"
    );
    assert!(!env.sys("/home/user/.config/a").exists());
    // The genuine entry still restores.
    let out = restore(&env, &Config::default(), &sel(&rep.batch), false).unwrap();
    assert!(out[0].ok, "{out:?}");
}

/// A genuine trash entry rewritten to point into `root` (in-system).
fn entry_in(env: &Env, root: &str) -> HistoryEntry {
    let mut e = read_history(env).unwrap().0[0].clone();
    let tr = e.trashed.as_mut().unwrap();
    let name = tr.files_path.file_name().unwrap().to_os_string();
    let mut info = name.clone();
    info.push(".trashinfo");
    tr.files_path = Path::new(root).join("files").join(&name);
    tr.info_path = Path::new(root).join("info").join(info);
    e
}

fn mode(t: &FixtureTree, rel: &str, m: u32) {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(t.dir.path().join(rel), fs::Permissions::from_mode(m)).unwrap();
}

fn topdir_fixture() -> (FixtureTree, Env) {
    let t = FixtureTree::new();
    t.write("home/user/.config/a/x", b"A");
    let env = t.env();
    run(&env, vec![trash("/home/user/.config/a")]);
    let uid = env.uid;
    t.mkdir(&format!("home/.Trash-{uid}/files"));
    t.mkdir(&format!("home/.Trash-{uid}/info"));
    mode(&t, &format!("home/.Trash-{uid}"), 0o700);
    (t, env)
}

#[test]
fn topdir_trash_needs_private_owner_dir() {
    let (t, env) = topdir_fixture();
    let cfg = Config::default();
    let uid = env.uid;
    let root = format!("/home/.Trash-{uid}");
    let e = entry_in(&env, &root);
    assert!(valid_trash(&env, &cfg, &e).is_some());
    // Mode 0755.
    mode(&t, &format!("home/.Trash-{uid}"), 0o755);
    assert!(valid_trash(&env, &cfg, &e).is_none());
    mode(&t, &format!("home/.Trash-{uid}"), 0o700);
    assert!(valid_trash(&env, &cfg, &e).is_some());
    // Wrong owner.
    let mut other = env.clone();
    other.uid = uid.wrapping_add(1);
    let e2 = entry_in(&other, &format!("/home/.Trash-{}", other.uid));
    assert!(valid_trash(&other, &cfg, &e2).is_none());
    // `files` a symlink.
    let files = t.dir.path().join(format!("home/.Trash-{uid}/files"));
    fs::remove_dir(&files).unwrap();
    t.mkdir("home/elsewhere");
    std::os::unix::fs::symlink(t.dir.path().join("home/elsewhere"), &files).unwrap();
    assert!(valid_trash(&env, &cfg, &e).is_none());
    fs::remove_file(&files).unwrap();
    t.mkdir(&format!("home/.Trash-{uid}/files"));
    assert!(valid_trash(&env, &cfg, &e).is_some());
    // The root itself a symlink.
    let real = t.dir.path().join(format!("home/.Trash-{uid}"));
    fs::rename(&real, t.dir.path().join("home/moved")).unwrap();
    std::os::unix::fs::symlink(t.dir.path().join("home/moved"), &real).unwrap();
    assert!(valid_trash(&env, &cfg, &e).is_none());
    // A name that merely looks like a trash dir, with no directory.
    let e3 = entry_in(&env, "/home/user/.Trash-0");
    assert!(valid_trash(&env, &cfg, &e3).is_none());
}

#[test]
fn shared_dot_trash_needs_sticky() {
    let (t, env) = topdir_fixture();
    let cfg = Config::default();
    let uid = env.uid;
    t.mkdir(&format!("home/.Trash/{uid}/files"));
    t.mkdir(&format!("home/.Trash/{uid}/info"));
    mode(&t, &format!("home/.Trash/{uid}"), 0o700);
    let e = entry_in(&env, &format!("/home/.Trash/{uid}"));
    // `.Trash` without the sticky bit.
    mode(&t, "home/.Trash", 0o777);
    assert!(valid_trash(&env, &cfg, &e).is_none());
    mode(&t, "home/.Trash", 0o1777);
    assert!(valid_trash(&env, &cfg, &e).is_some());
    // A symlinked `.Trash`.
    let real = t.dir.path().join("home/.Trash");
    fs::rename(&real, t.dir.path().join("home/.Trash-real")).unwrap();
    std::os::unix::fs::symlink(t.dir.path().join("home/.Trash-real"), &real).unwrap();
    assert!(valid_trash(&env, &cfg, &e).is_none());
}

#[test]
fn home_trash_outside_home_not_restorable() {
    let t = FixtureTree::new();
    t.write("home/user/.config/a/x", b"A");
    let env = t.env();
    run(&env, vec![trash("/home/user/.config/a")]);
    let mut env2 = env.clone();
    env2.data_home = t.dir.path().join("data");
    let e = entry_in(&env, "/data/Trash");
    assert!(valid_trash(&env2, &Config::default(), &e).is_none());
}

#[test]
fn restore_reports_commands_not_undoable() {
    let t = FixtureTree::new();
    t.write_exec("usr/bin/flatpak", b"#!/bin/sh\n");
    let env = t.env();
    let runner = crate::testing::FakeRunner::new().on(
        "flatpak",
        &["uninstall"],
        crate::command::CmdOutput {
            status: 0,
            stdout: String::new(),
            stderr: String::new(),
            timed_out: false,
        },
    );
    let plan = Plan {
        app_id: None,
        kind: None,
        actions: vec![PlannedAction {
            action: Action::UserCommand {
                program: "flatpak".into(),
                args: vec!["uninstall".into()],
            },
            size: 0,
            fingerprint: crate::safety::Fingerprint {
                dev: 0,
                ino: 0,
                is_symlink: false,
                ctime: 0,
                ctime_nsec: 0,
            },
            scope: crate::safety::Scope::Home,
            depends_on: None,
            depends_on_plan: None,
            size_known: true,
            reason: "test".into(),
        }],
        total_size: 0,
        notes: Vec::new(),
    };
    let rep = crate::executor::execute_with(&env, &Config::default(), &plan, &runner);
    assert!(rep.results[0].ok);
    // By entry id: reported as not undoable.
    let id = rep.results[0].history_id.clone();
    let out = restore(&env, &Config::default(), &sel(&id), false).unwrap();
    assert_eq!(out.len(), 1);
    assert!(!out[0].ok);
    assert_eq!(out[0].error.as_deref(), Some("commands cannot be undone"));
    // By batch: commands are skipped like other no-op entries.
    let out = restore(&env, &Config::default(), &sel(&rep.batch), false).unwrap();
    assert_eq!(out.len(), 1);
    assert!(out[0].ok, "{out:?}");
    assert_eq!(out[0].error.as_deref(), Some(NOTHING_RESTORABLE));
}

#[test]
fn restore_hint_names_the_package_manager() {
    let entry = |source: &str, app: &str, pkgs: &str| -> HistoryEntry {
        let src = if source.is_empty() {
            String::new()
        } else {
            format!(r#","source":"{source}""#)
        };
        serde_json::from_str(&format!(
            r#"{{"id":"x","batch":"b","time":"t","action":"package-remove","path":"/usr/bin/x",
                "trashed":null,"backup":null,"size":0,"ok":true,"error":null,"app_id":"{app}",
                "packages":{pkgs}{src}}}"#
        ))
        .unwrap()
    };
    let two = r#"[["foo","1"],["libfoo","2"]]"#;
    let hint = |e: &HistoryEntry| reinstall_hint(e);
    assert_eq!(
        hint(&entry("", "pacman:foo", two)),
        "packages cannot be restored by crumbs; reinstall with: \
             pacman -S foo && pacman -S --asdeps libfoo"
    );
    assert_eq!(
        hint(&entry("dpkg", "dpkg:foo", two)),
        "packages cannot be restored by crumbs; reinstall with: \
             apt-get install foo && apt-get install libfoo && apt-mark auto libfoo"
    );
    assert_eq!(
        hint(&entry("rpm", "rpm:foo", two)),
        "packages cannot be restored by crumbs; reinstall with: dnf install foo libfoo"
    );
    assert_eq!(
        hint(&entry("snap", "snap:foo", r#"[["foo","1"]]"#)),
        "packages cannot be restored by crumbs; reinstall with: snap install foo"
    );
}

#[test]
fn restore_refuses_packages_with_reinstall_hint() {
    let t = FixtureTree::new();
    t.write_exec("usr/bin/pacman", b"#!/bin/sh\n");
    t.write_exec("usr/bin/flatpak", b"#!/bin/sh\n");
    let env = t.env();
    let unfp = |action| PlannedAction {
        action,
        size: 0,
        fingerprint: crate::safety::Fingerprint {
            dev: 0,
            ino: 0,
            is_symlink: false,
            ctime: 0,
            ctime_nsec: 0,
        },
        scope: crate::safety::Scope::Home,
        depends_on: None,
        depends_on_plan: None,
        size_known: true,
        reason: "test".into(),
    };
    let plan = Plan {
        app_id: Some("pacman:foo".into()),
        kind: None,
        actions: vec![
            unfp(Action::PackageRemove {
                source: crate::model::SourceKind::Pacman,
                names: vec!["foo".into(), "libfoo".into()],
                versions: vec!["1.0-1".into(), "2.0-1".into()],
                program: "pacman".into(),
                args: vec!["-Rns".into(), "--noconfirm".into(), "foo".into()],
            }),
            unfp(Action::PrivilegedCommand {
                program: "flatpak".into(),
                args: vec!["uninstall".into()],
            }),
        ],
        total_size: 0,
        notes: Vec::new(),
    };
    let esc = crate::privilege::Escalation {
        kind: crate::privilege::EscalationKind::Pkexec,
        program: "/usr/bin/pkexec".into(),
        requires_tty: false,
    };
    let rep = crate::executor::execute_full(
        &env,
        &Config::default(),
        &plan,
        &crate::testing::FakeRunner::new(),
        &crate::testing::FakePrivilegedRunner::default(),
        Some(&esc),
        &|_, _, _| Ok(Vec::new()),
    );
    assert!(rep.results.iter().all(|r| r.ok), "{rep:?}");
    let by_id = |i: usize| {
        let id = rep.results[i].history_id.clone();
        let out = restore(&env, &Config::default(), &sel(&id), false).unwrap();
        assert_eq!(out.len(), 1);
        assert!(!out[0].ok);
        out[0].error.clone().unwrap()
    };
    assert_eq!(
        by_id(0),
        "packages cannot be restored by crumbs; reinstall with: \
             pacman -S foo && pacman -S --asdeps libfoo"
    );
    assert_eq!(by_id(1), "commands cannot be undone");
    // An entry that names no packages gives no dangling command.
    let mut e = read_history(&env).unwrap().0.remove(0);
    for packages in [None, Some(Vec::new())] {
        e.packages = packages;
        assert_eq!(
            restore_one(&env, &Config::default(), &e, true, "1"),
            Err("packages cannot be restored by crumbs".to_string())
        );
    }
    // A removal that failed part-way still says how to reinstall what
    // it removed; without an app (orphans) everything is a dependency.
    let mut e = read_history(&env).unwrap().0.remove(0);
    e.ok = false;
    e.packages = Some(vec![("libfoo".into(), "2.0-1".into())]);
    assert_eq!(
        restore_one(&env, &Config::default(), &e, true, "1"),
        Err(
            "the original removal only partly ran; packages cannot be restored by \
                 crumbs; reinstall with: pacman -S --asdeps libfoo"
                .to_string()
        )
    );
    e.packages = Some(Vec::new());
    assert_eq!(
        restore_one(&env, &Config::default(), &e, true, "1"),
        Err("the original action failed; nothing to restore".to_string())
    );
    e.ok = true;
    e.app_id = None;
    e.packages = Some(vec![
        ("foo".into(), "1.0-1".into()),
        ("libfoo".into(), "2.0-1".into()),
    ]);
    assert_eq!(
        restore_one(&env, &Config::default(), &e, true, "1"),
        Err("packages cannot be restored by crumbs; reinstall with: \
                 pacman -S --asdeps foo libfoo"
            .to_string())
    );
    // A batch restore skips both.
    let out = restore(&env, &Config::default(), &sel(&rep.batch), false).unwrap();
    assert_eq!(out.len(), 1);
    assert!(out[0].ok, "{out:?}");
    assert_eq!(out[0].error.as_deref(), Some(NOTHING_RESTORABLE));
}

#[test]
fn restore_delete_only_batch_says_nothing_restorable() {
    let t = FixtureTree::new();
    t.write("home/user/.config/a/x", b"A");
    let env = t.env();
    let rep = run(
        &env,
        vec![Action::Delete {
            path: "/home/user/.config/a".into(),
        }],
    );
    assert!(rep.results[0].ok, "{rep:?}");
    let out = restore(&env, &Config::default(), &sel(&rep.batch), false).unwrap();
    assert_eq!(out.len(), 1);
    assert!(out[0].ok, "{out:?}");
    assert!(out[0].entry_id.is_empty());
    assert_eq!(out[0].error.as_deref(), Some(NOTHING_RESTORABLE));
    // An id that names nothing is still no match.
    let out = restore(&env, &Config::default(), &sel("1-0"), false).unwrap();
    assert!(!out[0].ok);
    assert_eq!(out[0].error.as_deref(), Some(NO_MATCH));
}

#[test]
fn bad_topdir_root_gets_a_permissions_message() {
    let (t, env) = topdir_fixture();
    let cfg = Config::default();
    let uid = env.uid;
    let e = entry_in(&env, &format!("/home/.Trash-{uid}"));
    mode(&t, &format!("home/.Trash-{uid}"), 0o755);
    let want = format!(
        "trash directory /home/.Trash-{uid} has unsafe permissions or owner \
             (needs a private 0700 directory owned by you)"
    );
    assert_eq!(restore_one(&env, &cfg, &e, true, "1"), Err(want));
    // A home trash outside the home gets it too.
    let mut env2 = env.clone();
    env2.data_home = t.dir.path().join("data");
    let e = entry_in(&env, "/data/Trash");
    assert_eq!(
        restore_one(&env2, &cfg, &e, true, "1"),
        Err(
            "trash directory /data/Trash has unsafe permissions or owner \
                 (needs a private 0700 directory owned by you)"
                .to_string()
        )
    );
    // Other invalid entries keep the generic message.
    let mut e = entry_in(&env, &format!("/home/.Trash-{uid}"));
    e.path = "/etc/passwd".into();
    assert_eq!(
        restore_one(&env, &cfg, &e, true, "1"),
        Err(INVALID_ENTRY.to_string())
    );
}
