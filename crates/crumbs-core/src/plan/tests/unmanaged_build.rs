use super::*;

#[test]
fn unmanaged_target_first_and_leftovers_depend_on_it() {
    use crate::removal::UnmanagedTarget;
    let t = FixtureTree::new();
    t.write("opt/inky/a", b"aaaa");
    t.write("home/user/.config/inky/b", b"bbbb");
    let env = t.env();
    let tg = |scope| UnmanagedTarget {
        path: "/opt/inky".into(),
        scope,
        fingerprint: safety::fingerprint(&env, Path::new("/opt/inky")).unwrap(),
        size: disk_usage(&env, Path::new("/opt/inky")),
        left_link: Some("/usr/bin/inky".into()),
        left_link_protected: true,
        home_link: None,
    };
    let home = finding(&env, "/home/user/.config/inky", FindingKind::Config);
    let app = App::new(SourceKind::Unmanaged, "inky.desktop");
    let o = PlanOptions::default();
    let plan = build_plan_unmanaged(
        &env,
        Some(&app),
        Some(&tg(Scope::SystemTree)),
        std::slice::from_ref(&home),
        &o,
    )
    .unwrap();
    assert_eq!(kinds(&plan), ["delete", "trash"]);
    assert_eq!(plan.actions[0].scope, Scope::SystemTree);
    assert_eq!(plan.actions[1].depends_on, Some(0));
    assert!(plan.notes.iter().any(|n| n == NOTE_SYSTEM_DELETE));
    assert!(
        plan.notes
            .iter()
            .any(|n| n == "/usr/bin/inky is left in place (protected path)")
    );
    // Findings at or below the target go with it; a sibling stays.
    t.write("opt/inkyfoo/x", b"x");
    let below = finding(&env, "/opt/inky/a", FindingKind::SystemData);
    let sibling = finding(&env, "/opt/inkyfoo", FindingKind::SystemData);
    let p = build_plan_unmanaged(
        &env,
        Some(&app),
        Some(&tg(Scope::SystemTree)),
        &[below, sibling],
        &o,
    )
    .unwrap();
    assert_eq!(p.actions.len(), 2);
    assert_eq!(p.actions[1].action.path(), Some(Path::new("/opt/inkyfoo")));
    // A link in a home location is its own action, after the target.
    let mut with_link = tg(Scope::SystemTree);
    with_link.left_link = None;
    with_link.home_link = Some((
        "/home/user/.local/bin/inky".into(),
        crate::safety::Fingerprint::default(),
    ));
    let p = build_plan_unmanaged(&env, Some(&app), Some(&with_link), &[], &o).unwrap();
    assert_eq!(kinds(&p), ["delete", "trash"]);
    assert_eq!(p.actions[1].depends_on, Some(0));
    assert_eq!(p.actions[1].scope, Scope::Home);
    assert!(!p.notes.iter().any(|n| n.contains("left in place")));
    // A home target is trashed, or deleted with --permanent.
    let p = build_plan_unmanaged(&env, Some(&app), Some(&tg(Scope::Home)), &[], &o).unwrap();
    assert_eq!(kinds(&p), ["trash"]);
    let perm = PlanOptions {
        permanent: true,
        ..o
    };
    let p = build_plan_unmanaged(&env, Some(&app), Some(&tg(Scope::Home)), &[], &perm).unwrap();
    assert_eq!(kinds(&p), ["delete"]);
    // Leftovers only: the target is ignored.
    let lo = PlanOptions {
        leftovers_only: true,
        ..o
    };
    let p =
        build_plan_unmanaged(&env, Some(&app), Some(&tg(Scope::SystemTree)), &[home], &lo).unwrap();
    assert_eq!(kinds(&p), ["trash"]);
    assert!(p.notes.iter().any(|n| n == NOTE_APP_STAYS));
}

#[test]
fn build_plan_system_finding_is_root_delete() {
    let t = FixtureTree::new();
    t.write("opt/inky/a", b"aaaa");
    t.write("home/user/.config/inky/b", b"bbbb");
    let env = t.env();
    let mut sys = finding(&env, "/opt/inky", FindingKind::SystemData);
    sys.scope = Scope::SystemTree;
    sys.preselected = false;
    let home = finding(&env, "/home/user/.config/inky", FindingKind::Config);
    let fs = [home, sys.clone()];
    let app = App::new(SourceKind::Pacman, "inky");
    let plan = build_plan(&env, Some(&app), &fs, &PlanOptions::default()).unwrap();
    assert_eq!(kinds(&plan), ["trash", "delete"]);
    assert_eq!(plan.actions[0].scope, Scope::Home);
    let a = &plan.actions[1];
    assert_eq!(a.scope, Scope::SystemTree);
    assert_eq!(a.action.path(), Some(Path::new("/opt/inky")));
    assert_eq!(a.fingerprint, sys.fingerprint);
    assert!(plan.notes.iter().any(|n| n == NOTE_SYSTEM_DELETE));
    assert_eq!(
        plan.notes
            .iter()
            .filter(|n| *n == NOTE_SYSTEM_DELETE)
            .count(),
        1
    );
    // Without a system finding the note is absent.
    let plan = build_plan(&env, Some(&app), &fs[..1], &PlanOptions::default()).unwrap();
    assert!(!plan.notes.iter().any(|n| n == NOTE_SYSTEM_DELETE));
    // A size that could not be read is carried as unknown.
    assert!(plan.actions.iter().all(|a| a.size_known));
    let mut unread = sys.clone();
    unread.size = crate::size::DiskUsage {
        bytes: 0,
        errors: 1,
        ..Default::default()
    };
    let plan = build_plan(&env, Some(&app), &[unread], &PlanOptions::default()).unwrap();
    assert!(!plan.actions[0].size_known);
    assert!(size_known(&crate::size::DiskUsage {
        bytes: 4096,
        errors: 1,
        ..Default::default()
    }));
}

#[test]
fn plan_trash_default_delete_permanent() {
    let t = FixtureTree::new();
    t.write("home/user/.config/foo/a", b"aaaa");
    t.write("home/user/.cache/foo/b", b"bbbb");
    let env = t.env();
    let fs = [
        finding(&env, "/home/user/.config/foo", FindingKind::Config),
        finding(&env, "/home/user/.cache/foo", FindingKind::Cache),
    ];
    let app = App::new(SourceKind::Pacman, "foo");
    let plan = build_plan(&env, Some(&app), &fs, &PlanOptions::default()).unwrap();
    assert_eq!(kinds(&plan), ["trash", "trash"]);
    assert_eq!(plan.app_id.as_deref(), Some("pacman:foo"));
    assert_eq!(plan.kind.as_deref(), Some("uninstall"));
    assert_eq!(plan.notes, [NOTE_APP_STAYS]);
    assert_eq!(plan.total_size, fs[0].size.bytes + fs[1].size.bytes);
    assert!(plan.total_size > 0);
    assert_eq!(
        plan.actions[0].action.path(),
        Some(Path::new("/home/user/.config/foo"))
    );
    assert_eq!(plan.actions[0].fingerprint, fs[0].fingerprint);
    assert_eq!(plan.actions[0].reason, fs[0].reason);

    let opts = PlanOptions {
        permanent: true,
        keep_data: false,
        ..PlanOptions::default()
    };
    let plan = build_plan(&env, None, &fs, &opts).unwrap();
    assert_eq!(kinds(&plan), ["delete", "delete"]);
    assert_eq!(plan.app_id, None);
    assert_eq!(plan.kind.as_deref(), Some("orphans"));
    assert!(plan.notes.is_empty());

    let json = serde_json::to_value(&plan.actions[0]).unwrap();
    assert_eq!(json["action"]["type"], "delete");
    assert_eq!(json["action"]["path"], "/home/user/.config/foo");
    assert!(json.get("fingerprint").is_none());
}

#[test]
fn plan_keep_data_only_cache() {
    let t = FixtureTree::new();
    t.write("home/user/.config/foo/a", b"a");
    t.write("home/user/.cache/foo/b", b"b");
    t.write("home/user/.local/share/foo/c", b"c");
    t.write("home/user/.config/mimeapps.list", b"x=foo.desktop\n");
    let env = t.env();
    let mut mime = finding(
        &env,
        "/home/user/.config/mimeapps.list",
        FindingKind::MimeAssoc,
    );
    mime.mime_lines = vec!["x=foo.desktop".into()];
    let fs = [
        finding(&env, "/home/user/.config/foo", FindingKind::Config),
        finding(&env, "/home/user/.cache/foo", FindingKind::Cache),
        finding(&env, "/home/user/.local/share/foo", FindingKind::Data),
        mime,
    ];
    let opts = PlanOptions {
        permanent: false,
        keep_data: true,
        ..PlanOptions::default()
    };
    let plan = build_plan(&env, None, &fs, &opts).unwrap();
    assert_eq!(plan.actions.len(), 1);
    assert_eq!(
        plan.actions[0].action,
        Action::Trash {
            path: "/home/user/.cache/foo".into()
        }
    );
    assert_eq!(plan.total_size, fs[1].size.bytes);
}

#[test]
fn plan_leftovers_only_skips_appimage_file() {
    let t = FixtureTree::new();
    t.write("home/user/.config/foo/a", b"a");
    let env = t.env();
    // No location: a full plan fails, a leftovers-only plan never looks.
    let mut app = App::new(SourceKind::AppImage, "Foo.AppImage");
    app.location = None;
    let fs = [finding(&env, "/home/user/.config/foo", FindingKind::Config)];
    let opts = PlanOptions {
        leftovers_only: true,
        ..PlanOptions::default()
    };
    let plan = build_plan(&env, Some(&app), &fs, &opts).unwrap();
    assert_eq!(kinds(&plan), ["trash"]);
    assert_eq!(
        plan.actions[0].action.path(),
        Some(Path::new("/home/user/.config/foo"))
    );
    assert_eq!(plan.notes, [NOTE_APP_STAYS]);
    assert_eq!(plan.app_id.as_deref(), Some("appimage:Foo.AppImage"));
}

#[test]
fn plan_mime_is_edit() {
    let t = FixtureTree::new();
    t.write(
        "home/user/.config/mimeapps.list",
        b"a=foo.desktop\nb=bar.desktop\n",
    );
    let env = t.env();
    let mut mime = finding(
        &env,
        "/home/user/.config/mimeapps.list",
        FindingKind::MimeAssoc,
    );
    mime.mime_lines = vec!["a=foo.desktop".into()];
    for permanent in [false, true] {
        let opts = PlanOptions {
            permanent,
            keep_data: false,
            leftovers_only: false,
        };
        let plan = build_plan(&env, None, std::slice::from_ref(&mime), &opts).unwrap();
        assert_eq!(
            plan.actions[0].action,
            Action::EditFile {
                path: "/home/user/.config/mimeapps.list".into(),
                remove_lines: vec!["a=foo.desktop".into()],
            }
        );
        assert_eq!(plan.actions[0].size, 0);
        assert_eq!(plan.total_size, 0);
    }
    let plan = build_plan(&env, None, &[mime], &PlanOptions::default()).unwrap();
    let json = serde_json::to_value(&plan.actions[0].action).unwrap();
    assert_eq!(json["type"], "edit-file");
    assert_eq!(json["remove_lines"][0], "a=foo.desktop");
}

#[test]
fn plan_appimage_includes_file_first() {
    let t = FixtureTree::new();
    t.appimage("home/user/Applications/Foo.AppImage");
    t.write("home/user/.config/foo/a", b"a");
    let env = t.env();
    let loc = PathBuf::from("/home/user/Applications/Foo.AppImage");
    let mut app = App::new(SourceKind::AppImage, "Foo.AppImage");
    app.id = "appimage:~/Applications/Foo.AppImage".into();
    app.location = Some(loc.clone());
    let fs = [finding(&env, "/home/user/.config/foo", FindingKind::Config)];
    let opts = PlanOptions {
        permanent: false,
        keep_data: true,
        ..PlanOptions::default()
    };
    let plan = build_plan(&env, Some(&app), &fs, &opts).unwrap();
    assert_eq!(plan.actions.len(), 1, "keep_data drops the config");
    assert_eq!(plan.actions[0].action, Action::Trash { path: loc.clone() });
    assert_eq!(plan.actions[0].reason, REASON_APPIMAGE);
    assert_eq!(
        plan.actions[0].fingerprint,
        safety::fingerprint(&env, &loc).unwrap()
    );
    assert!(plan.actions[0].size > 0);
    assert!(plan.notes.is_empty());

    let plan = build_plan(&env, Some(&app), &fs, &PlanOptions::default()).unwrap();
    assert_eq!(kinds(&plan), ["trash", "trash"]);
    assert_eq!(plan.actions[0].action.path(), Some(loc.as_path()));

    app.location = None;
    assert!(build_plan(&env, Some(&app), &fs, &PlanOptions::default()).is_err());
}
