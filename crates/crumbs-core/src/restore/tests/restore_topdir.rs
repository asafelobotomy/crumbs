use super::*;

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
            group: None,
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
        group: None,
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
