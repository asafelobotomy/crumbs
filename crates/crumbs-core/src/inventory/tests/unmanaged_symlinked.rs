use super::*;

#[test]
fn unmanaged_via_discover_and_status_order() {
    let t = FixtureTree::new();
    firefox(&t);
    t.write_exec("/opt/foo/foo", b"");
    t.desktop(
        "/usr/share/applications/foo.desktop",
        "[Desktop Entry]\nType=Application\nName=Zed Foo\nExec=/opt/foo/foo\n",
    );
    t.appimage("/home/user/Applications/Tool-1.0-x86_64.AppImage");
    let inv = discover(
        &t.env(),
        &FakeRunner::new(),
        &ListOptions::default(),
        &Config::default(),
    );
    let kinds: Vec<_> = inv.statuses.iter().map(|s| s.source).collect();
    assert_eq!(
        kinds,
        [
            SourceKind::Pacman,
            SourceKind::Dpkg,
            SourceKind::Rpm,
            SourceKind::Flatpak,
            SourceKind::Snap,
            SourceKind::AppImage,
            SourceKind::Unmanaged
        ]
    );
    let counts: Vec<_> = inv.statuses.iter().map(|s| s.app_count).collect();
    assert_eq!(counts, [1, 0, 0, 0, 0, 1, 1]);
    assert_eq!(
        inv.statuses[1].skipped_reason.as_deref(),
        Some("no dpkg database")
    );
    assert_eq!(
        inv.statuses[2].skipped_reason.as_deref(),
        Some("rpm not installed")
    );
    assert_eq!(
        inv.statuses[3].skipped_reason.as_deref(),
        Some("not installed")
    );
    assert!(!inv.statuses[3].available);
    let ids: Vec<_> = inv.apps.iter().map(|a| a.id.as_str()).collect();
    assert_eq!(
        ids,
        [
            "pacman:firefox",
            "appimage:~/Applications/Tool-1.0-x86_64.AppImage",
            "unmanaged:foo.desktop"
        ]
    );
    assert!(inv.errors.is_empty());
}

#[test]
fn unmanaged_skipped_when_package_db_has_errors() {
    let t = FixtureTree::new();
    firefox(&t);
    t.write(
        "/var/lib/pacman/local/nofiles-1-1/desc",
        b"%NAME%\nnofiles\n\n%VERSION%\n1-1\n\n",
    );
    t.write_exec("/opt/foo/foo", b"");
    t.desktop(
        "/usr/share/applications/foo.desktop",
        "[Desktop Entry]\nType=Application\nName=Foo\nExec=/opt/foo/foo\n",
    );
    let inv = discover(
        &t.env(),
        &FakeRunner::new(),
        &ListOptions::default(),
        &Config::default(),
    );
    assert!(!inv.errors.is_empty());
    let st = inv.statuses.last().unwrap();
    assert_eq!(st.source, SourceKind::Unmanaged);
    assert!(!st.available);
    assert_eq!(
        st.skipped_reason.as_deref(),
        Some("package database incomplete")
    );
    assert!(inv.apps.iter().all(|a| a.source != SourceKind::Unmanaged));
}

#[test]
fn unmanaged_skipped_without_package_db() {
    let t = FixtureTree::new();
    t.write_exec("/opt/foo/foo", b"");
    t.desktop(
        "/usr/share/applications/foo.desktop",
        "[Desktop Entry]\nType=Application\nName=Foo\nExec=/opt/foo/foo\n",
    );
    let inv = discover(
        &t.env(),
        &FakeRunner::new(),
        &ListOptions::default(),
        &Config::default(),
    );
    let st = inv.statuses.last().unwrap();
    assert_eq!(st.source, SourceKind::Unmanaged);
    assert_eq!(
        st.skipped_reason.as_deref(),
        Some("no supported package database found")
    );
    assert_eq!(st.app_count, 0);
    assert!(inv.apps.iter().all(|a| a.source != SourceKind::Unmanaged));
    assert_eq!(
        inv.statuses[0].skipped_reason.as_deref(),
        Some("no pacman database")
    );
}

#[test]
fn symlinked_exec_links_appimage_and_is_not_unmanaged() {
    let t = FixtureTree::new();
    pacman_db(&t);
    t.appimage("/home/user/Applications/Foo-1.0-x86_64.AppImage");
    t.symlink(
        "/home/user/.local/share/foo/appimage",
        "../../../Applications/Foo-1.0-x86_64.AppImage",
    );
    t.desktop(
        "/home/user/.local/share/applications/foo.desktop",
        "[Desktop Entry]\nType=Application\nName=Foo\nExec=~/.local/share/foo/appimage\n",
    );
    let inv = discover(
        &t.env(),
        &FakeRunner::new(),
        &ListOptions::default(),
        &Config::default(),
    );
    let ids: Vec<_> = inv.apps.iter().map(|a| a.id.as_str()).collect();
    assert_eq!(ids, ["appimage:~/Applications/Foo-1.0-x86_64.AppImage"]);
    assert_eq!(inv.apps[0].desktop_ids, ["foo.desktop"]);
}

#[test]
fn failing_source_does_not_stop_others() {
    let t = FixtureTree::new();
    pacman_db(&t);
    firefox(&t);
    t.write_exec("/usr/bin/flatpak", b"");
    t.appimage("/home/user/Applications/Tool-1.0-x86_64.AppImage");
    let inv = discover(
        &t.env(),
        &FakeRunner::new(),
        &ListOptions::default(),
        &Config::default(),
    );
    assert_eq!(inv.errors.len(), 1);
    assert_eq!(inv.errors[0].source, SourceKind::Flatpak);
    assert!(inv.apps.iter().any(|a| a.source == SourceKind::Pacman));
    assert!(inv.apps.iter().any(|a| a.source == SourceKind::AppImage));
    assert!(inv.statuses[3].available);
}

#[test]
fn flatpak_claims_its_desktop_entry() {
    let t = FixtureTree::new();
    pacman_db(&t);
    t.write_exec("/usr/bin/flatpak", b"");
    t.write_exec("/usr/bin/flatpak-wrapper", b"");
    t.desktop(
        "/usr/share/applications/org.x.Y.desktop",
        "[Desktop Entry]\nType=Application\nName=Y\nExec=/usr/bin/flatpak-wrapper\n",
    );
    let runner = FakeRunner::new().on("flatpak", &FLATPAK_ARGS, ok("org.x.Y\tY\t1\tuser\t1 MB\n"));
    let inv = discover(
        &t.env(),
        &runner,
        &ListOptions::default(),
        &Config::default(),
    );
    assert_eq!(inv.apps.len(), 1);
    assert_eq!(inv.apps[0].source, SourceKind::Flatpak);
}

#[test]
fn find_app_precedence() {
    let t = FixtureTree::new();
    firefox(&t);
    t.write_exec("/usr/bin/flatpak", b"");
    let runner = FakeRunner::new().on(
        "flatpak",
        &FLATPAK_ARGS,
        ok("org.mozilla.firefox\tFirefox\t1\tsystem\t1 MB\n"),
    );
    let inv = discover(
        &t.env(),
        &runner,
        &ListOptions::default(),
        &Config::default(),
    );
    assert_eq!(inv.apps.len(), 2);
    assert_eq!(found(find_app(&inv, "firefox")).id, "pacman:firefox");
    assert_eq!(
        found(find_app(&inv, "org.mozilla.firefox")).id,
        "flatpak:system/org.mozilla.firefox"
    );
    assert_eq!(
        found(find_app(&inv, "org.mozilla.firefox.desktop")).source,
        SourceKind::Flatpak
    );
    assert_eq!(
        found(find_app(&inv, "firefox.desktop")).id,
        "pacman:firefox"
    );
    assert_eq!(
        found(find_app(&inv, "FIREFOX WEB BROWSER")).id,
        "pacman:firefox"
    );
    assert!(matches!(find_app(&inv, "nope"), Resolved::NotFound));
}

#[test]
fn find_app_rdns_and_normalized() {
    let mut fp = app(SourceKind::Flatpak, "org.x.Y", "Why");
    fp.rdns_id = Some("org.x.Y".into());
    let tool = app(SourceKind::AppImage, "Tool.AppImage", "My-Tool");
    let inv = inv(vec![fp, tool]);
    assert_eq!(
        found(find_app(&inv, "my tool")).id,
        "appimage:Tool.AppImage"
    );
    assert_eq!(found(find_app(&inv, "mytool")).id, "appimage:Tool.AppImage");
    assert_eq!(found(find_app(&inv, "org.x.y")).id, "flatpak:org.x.Y");
}

#[test]
fn find_app_exact_skips_normalized() {
    let tool = app(SourceKind::AppImage, "Tool.AppImage", "My-Tool");
    let inv = inv(vec![tool]);
    assert!(matches!(
        find_app_exact(&inv, "my tool"),
        Resolved::NotFound
    ));
    assert!(matches!(find_app_exact(&inv, "mytool"), Resolved::NotFound));
    assert_eq!(
        found(find_app_exact(&inv, "MY-TOOL")).id,
        "appimage:Tool.AppImage"
    );
    assert_eq!(
        found(find_app_exact(&inv, "appimage:Tool.AppImage")).id,
        "appimage:Tool.AppImage"
    );
    assert_eq!(
        found(find_app(&inv, "my tool")).id,
        "appimage:Tool.AppImage"
    );
}

#[test]
fn find_by_desktop_id_matches_only_launchers() {
    let mut ff = app(SourceKind::Pacman, "firefox", "Firefox Web Browser");
    ff.desktop_ids = vec!["firefox.desktop".into()];
    let mut fp = app(SourceKind::Flatpak, "org.mozilla.firefox", "Firefox");
    fp.desktop_ids = vec!["org.mozilla.firefox.desktop".into()];
    let mut kde = app(SourceKind::Pacman, "dolphin", "Dolphin");
    kde.desktop_ids = vec![
        "kde-org.kde.dolphin.desktop".into(),
        "dolphin-root.desktop".into(),
    ];
    let named = app(SourceKind::Pacman, "named.desktop", "named.desktop");
    let inv = inv(vec![ff, fp, kde, named]);
    assert_eq!(
        found(find_app_by_desktop_id(&inv, "firefox.desktop")).id,
        "pacman:firefox"
    );
    assert_eq!(
        found(find_app_by_desktop_id(&inv, "org.mozilla.firefox.desktop")).source,
        SourceKind::Flatpak
    );
    assert_eq!(
        found(find_app_by_desktop_id(&inv, "dolphin-root.desktop")).id,
        "pacman:dolphin"
    );
    // Only the full id of a launcher: no extension added, no app id, name
    // or display name, no case folding.
    for miss in [
        "firefox",
        "org.mozilla.firefox",
        "pacman:firefox",
        "Firefox.desktop",
        "named.desktop",
        "nope.desktop",
    ] {
        assert!(
            matches!(find_app_by_desktop_id(&inv, miss), Resolved::NotFound),
            "{miss:?}"
        );
    }
}

#[test]
fn find_by_desktop_id_refuses_invalid_ids_and_reports_ambiguity() {
    let mut a = app(SourceKind::Pacman, "a", "A");
    a.desktop_ids = vec!["-x.desktop".into(), "shared.desktop".into()];
    let mut b = app(SourceKind::Flatpak, "b", "B");
    b.desktop_ids = vec!["shared.desktop".into()];
    let inv = inv(vec![a, b]);
    // Even an app that lists it can't be reached through an invalid id.
    assert!(matches!(
        find_app_by_desktop_id(&inv, "-x.desktop"),
        Resolved::NotFound
    ));
    match find_app_by_desktop_id(&inv, "shared.desktop") {
        Resolved::Ambiguous(v) => assert_eq!(v.len(), 2),
        other => panic!("expected Ambiguous, got {other:?}"),
    }
}
