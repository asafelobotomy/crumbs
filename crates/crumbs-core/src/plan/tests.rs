use super::*;
use crate::leftovers::Confidence;
use crate::testing::FixtureTree;

/// A finding for an existing in-system path, fingerprinted now.
pub(crate) fn finding(env: &Env, path: &str, kind: FindingKind) -> Finding {
    let p = PathBuf::from(path);
    Finding {
        size: disk_usage(env, &p),
        fingerprint: safety::fingerprint(env, &p).unwrap(),
        path: p,
        kind,
        confidence: Confidence::High,
        shared: false,
        reason: format!("reason for {path}"),
        preselected: true,
        scope: Scope::Home,
        mime_lines: Vec::new(),
    }
}

fn kinds(plan: &Plan) -> Vec<&'static str> {
    plan.actions
        .iter()
        .map(|a| match a.action {
            Action::Trash { .. } => "trash",
            Action::Delete { .. } => "delete",
            Action::EditFile { .. } => "edit",
            Action::UserCommand { .. } => "command",
            Action::PrivilegedCommand { .. } => "privileged-command",
            Action::PackageRemove { .. } => "package-remove",
        })
        .collect()
}

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

#[test]
fn appimage_leftovers_depend_on_the_file() {
    let t = FixtureTree::new();
    t.appimage("home/user/Applications/Foo.AppImage");
    t.write("home/user/.config/foo/a", b"a");
    t.write("home/user/.cache/foo/b", b"b");
    let env = t.env();
    let mut app = App::new(SourceKind::AppImage, "Foo.AppImage");
    app.location = Some("/home/user/Applications/Foo.AppImage".into());
    let fs = [
        finding(&env, "/home/user/.config/foo", FindingKind::Config),
        finding(&env, "/home/user/.cache/foo", FindingKind::Cache),
    ];
    let plan = build_plan(&env, Some(&app), &fs, &PlanOptions::default()).unwrap();
    let deps: Vec<Option<usize>> = plan.actions.iter().map(|a| a.depends_on).collect();
    assert_eq!(deps, [None, Some(0), Some(0)]);
    let json = serde_json::to_value(&plan.actions[0]).unwrap();
    assert!(json.get("depends_on").is_none());
    let json = serde_json::to_value(&plan.actions[1]).unwrap();
    assert_eq!(json["depends_on"], 0);

    // Leftovers only: the app stays, so nothing depends on anything.
    let opts = PlanOptions {
        leftovers_only: true,
        ..PlanOptions::default()
    };
    let plan = build_plan(&env, Some(&app), &fs, &opts).unwrap();
    assert!(plan.actions.iter().all(|a| a.depends_on.is_none()));
}

fn bitem(
    env: &Env,
    path: &str,
    action: crate::bloat::ItemAction,
    scope: Scope,
) -> crate::bloat::BloatItem {
    let p = PathBuf::from(path);
    crate::bloat::BloatItem {
        category: crate::bloat::Category::Cache,
        size: Some(disk_usage(env, &p).bytes),
        fingerprint: Some(safety::fingerprint(env, &p).unwrap()),
        path: Some(p),
        label: format!("label {path}"),
        rebuildable: true,
        preselected: true,
        note: None,
        action,
        scope,
        extra_paths: Vec::new(),
        app_id: None,
    }
}

fn command_item() -> crate::bloat::BloatItem {
    crate::bloat::BloatItem {
        category: crate::bloat::Category::FlatpakUnused,
        path: None,
        label: "remove unused runtimes".into(),
        size: None,
        rebuildable: false,
        preselected: true,
        note: Some("decided by flatpak".into()),
        action: crate::bloat::ItemAction::Command {
            program: "flatpak".into(),
            args: vec!["uninstall".into(), "--unused".into()],
        },
        scope: Scope::Home,
        fingerprint: None,
        extra_paths: Vec::new(),
        app_id: None,
    }
}

#[test]
fn clean_plan_maps_actions() {
    use crate::bloat::ItemAction;
    let t = FixtureTree::new();
    t.write("home/user/.cache/foo/a", b"aaaa");
    t.write("home/user/.cache/bar/b", b"bbbb");
    t.write("tmp/old", b"tttt");
    let env = t.env();
    let items = [
        bitem(
            &env,
            "/home/user/.cache/foo",
            ItemAction::Trash,
            Scope::Home,
        ),
        bitem(
            &env,
            "/home/user/.cache/bar",
            ItemAction::Delete,
            Scope::Home,
        ),
        bitem(&env, "/tmp/old", ItemAction::Delete, Scope::UserTemp),
        command_item(),
    ];
    let plan = build_clean_plan(&env, &items, &PlanOptions::default()).unwrap();
    assert_eq!(kinds(&plan), ["trash", "delete", "delete", "command"]);
    assert_eq!(plan.app_id, None);
    assert_eq!(plan.kind.as_deref(), Some("clean"));
    assert_eq!(plan.actions[2].scope, Scope::UserTemp);
    assert_eq!(plan.actions[0].fingerprint, items[0].fingerprint.unwrap());
    assert_eq!(plan.actions[0].reason, "label /home/user/.cache/foo");
    assert_eq!(plan.actions[3].reason, "decided by flatpak");
    assert_eq!(
        plan.actions[3].action,
        Action::UserCommand {
            program: "flatpak".into(),
            args: vec!["uninstall".into(), "--unused".into()],
        }
    );
    assert_eq!(plan.actions[3].size, 0);
    assert_eq!(plan.notes, [NOTE_TRASH_FREES_LATER, NOTE_PERMANENT_TEMP]);
    let json = serde_json::to_value(&plan.actions[3].action).unwrap();
    assert_eq!(json["type"], "user-command");
    assert_eq!(json["program"], "flatpak");
}

#[test]
fn trash_on_a_nearly_full_disk_says_so() {
    use crate::bloat::ItemAction;
    let t = FixtureTree::new();
    t.write("home/user/.cache/foo/a", b"aaaa");
    let mut env = t.env();
    let items = [bitem(
        &env,
        "/home/user/.cache/foo",
        ItemAction::Trash,
        Scope::Home,
    )];
    // 97% used, 9.4 GiB free: the trash would free nothing now.
    let total = 239 * 1024 * 1024 * 1024_u64;
    let avail = 9 * 1024 * 1024 * 1024 + 400 * 1024 * 1024;
    env.set_fs_space(Some((avail, total)));
    let plan = build_clean_plan(&env, &items, &PlanOptions::default()).unwrap();
    assert_eq!(
        plan.notes,
        [
            "the disk is 97% full (9.4 GiB free): moving to the trash frees nothing \
              until the trash is emptied; use --permanent to free the space now"
        ]
    );
    // 50% used: the usual note.
    env.set_fs_space(Some((total / 2, total)));
    let plan = build_clean_plan(&env, &items, &PlanOptions::default()).unwrap();
    assert_eq!(plan.notes, [NOTE_TRASH_FREES_LATER]);
    // Deleting permanently on a full disk: no trash, no warning.
    env.set_fs_space(Some((avail, total)));
    let opts = PlanOptions {
        permanent: true,
        ..PlanOptions::default()
    };
    let plan = build_clean_plan(&env, &items, &opts).unwrap();
    assert!(
        plan.notes.iter().all(|n| !n.contains("full")),
        "{:?}",
        plan.notes
    );
}

#[test]
fn privileged_items_become_privileged_commands() {
    use crate::bloat::ItemAction;
    let mut item = command_item();
    item.category = crate::bloat::Category::Journal;
    item.scope = Scope::System;
    item.action = ItemAction::Privileged {
        program: "journalctl".into(),
        args: vec!["--vacuum-size=500M".into()],
    };
    let t = FixtureTree::new();
    let plan = build_clean_plan(&t.env(), &[item], &PlanOptions::default()).unwrap();
    assert_eq!(kinds(&plan), ["privileged-command"]);
    assert_eq!(
        plan.actions[0].action,
        Action::PrivilegedCommand {
            program: "journalctl".into(),
            args: vec!["--vacuum-size=500M".into()],
        }
    );
    assert_eq!(plan.actions[0].scope, Scope::System);
}

#[test]
fn clean_plan_trash_pairs() {
    use crate::bloat::ItemAction;
    let t = FixtureTree::new();
    t.write("home/user/.local/share/Trash/files/x", b"xxxx");
    t.write("home/user/.local/share/Trash/info/x.trashinfo", b"i");
    let env = t.env();
    let info = PathBuf::from("/home/user/.local/share/Trash/info/x.trashinfo");
    let mut item = bitem(
        &env,
        "/home/user/.local/share/Trash/files/x",
        ItemAction::Delete,
        Scope::HomeTrash,
    );
    item.extra_paths = vec![(info.clone(), safety::fingerprint(&env, &info).unwrap())];
    let plan = build_clean_plan(&env, &[item], &PlanOptions::default()).unwrap();
    assert_eq!(kinds(&plan), ["delete", "delete"]);
    assert_eq!(plan.actions[1].action, Action::Delete { path: info });
    assert_eq!(plan.actions[0].depends_on, None);
    assert_eq!(plan.actions[1].depends_on, Some(0));
    assert!(plan.actions.iter().all(|a| a.scope == Scope::HomeTrash));
    assert_eq!(plan.notes, [NOTE_PERMANENT_TRASH]);
    assert_eq!(plan.total_size, plan.actions[0].size);
}

#[test]
fn permanent_turns_trash_into_delete() {
    use crate::bloat::ItemAction;
    let t = FixtureTree::new();
    t.write("home/user/.cache/foo/a", b"aaaa");
    t.write("tmp/old", b"t");
    let env = t.env();
    let opts = PlanOptions {
        permanent: true,
        ..PlanOptions::default()
    };
    let items = [bitem(
        &env,
        "/home/user/.cache/foo",
        ItemAction::Trash,
        Scope::Home,
    )];
    let plan = build_clean_plan(&env, &items, &opts).unwrap();
    assert_eq!(kinds(&plan), ["delete"]);
    assert!(plan.notes.is_empty());

    // Temp and trash items are never trashed, permanent or not.
    for scope in [Scope::UserTemp, Scope::HomeTrash] {
        let bad = [bitem(&env, "/tmp/old", ItemAction::Trash, scope)];
        for o in [PlanOptions::default(), opts] {
            assert!(
                matches!(
                    build_clean_plan(&env, &bad, &o),
                    Err(CoreError::Parse { .. })
                ),
                "{scope:?}"
            );
        }
    }
}

#[test]
fn action_path_none_for_commands() {
    let cmd = Action::UserCommand {
        program: "flatpak".into(),
        args: vec![],
    };
    assert_eq!(cmd.path(), None);
    let trash = Action::Trash {
        path: PathBuf::from("/home/user/x"),
    };
    assert_eq!(trash.path(), Some(Path::new("/home/user/x")));
}

#[test]
fn privileged_actions_have_no_path_and_kebab_tags() {
    let cmd = Action::PrivilegedCommand {
        program: "flatpak".into(),
        args: vec!["uninstall".into()],
    };
    let pkg = Action::PackageRemove {
        source: SourceKind::Pacman,
        names: vec!["foo".into()],
        versions: vec!["1.0-1".into()],
        program: "pacman".into(),
        args: vec!["-Rns".into(), "--noconfirm".into(), "foo".into()],
    };
    assert_eq!(cmd.path(), None);
    assert_eq!(pkg.path(), None);
    let v = serde_json::to_value(&cmd).unwrap();
    assert_eq!(v["type"], "privileged-command");
    let v = serde_json::to_value(&pkg).unwrap();
    assert_eq!(v["type"], "package-remove");
    assert_eq!(v["source"], "pacman");
    assert_eq!(v["names"][0], "foo");
}

#[test]
fn scope_serialised_only_for_paths_outside_home() {
    let pa = |action: Action, scope: Scope| PlannedAction {
        action,
        size: 0,
        fingerprint: Fingerprint {
            dev: 0,
            ino: 0,
            is_symlink: false,
            ctime: 0,
            ctime_nsec: 0,
        },
        scope,
        depends_on: Some(0),
        depends_on_plan: None,
        size_known: true,
        reason: "r".into(),
    };
    let cmd = pa(
        Action::PrivilegedCommand {
            program: "flatpak".into(),
            args: Vec::new(),
        },
        Scope::System,
    );
    let v = serde_json::to_value(&cmd).unwrap();
    assert!(v.get("scope").is_none(), "{v}");
    assert_eq!(v["depends_on"], 0);
    assert_eq!(v["reason"], "r");
    let del = pa(
        Action::Delete {
            path: "/var/lib/systemd/coredump/core.1".into(),
        },
        Scope::System,
    );
    let v = serde_json::to_value(&del).unwrap();
    assert_eq!(v["scope"], serde_json::to_value(Scope::System).unwrap());
    let home = pa(trash_action(), Scope::Home);
    assert!(serde_json::to_value(&home).unwrap().get("scope").is_none());
}

fn trash_action() -> Action {
    Action::Trash {
        path: "/home/user/x".into(),
    }
}
