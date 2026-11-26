use super::*;

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
fn scope_serialised_only_for_paths_outside_home() {
    let pa = |action: Action, scope: Scope| PlannedAction {
        group: None,
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
