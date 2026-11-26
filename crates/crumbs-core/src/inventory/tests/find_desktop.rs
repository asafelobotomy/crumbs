use super::*;

#[test]
fn find_app_ambiguous() {
    let inv = inv(vec![
        app(SourceKind::Pacman, "firefox-a", "Firefox"),
        app(SourceKind::Flatpak, "org.mozilla.firefox", "Firefox"),
    ]);
    match find_app(&inv, "Firefox") {
        Resolved::Ambiguous(v) => assert_eq!(v.len(), 2),
        other => panic!("expected Ambiguous, got {other:?}"),
    }
}

#[test]
fn desktop_id_validation() {
    for good in [
        "firefox.desktop",
        "org.mozilla.firefox.desktop",
        "kde-org.kde.dolphin.desktop",
        "firefox_firefox.desktop",
        "x-.desktop",
        "A9.desktop",
    ] {
        assert!(valid_desktop_id(good), "{good:?}");
    }
    let long = format!("{}.desktop", "a".repeat(201));
    for bad in [
        "",
        "firefox",
        ".desktop",
        "-rf.desktop",
        "--desktop.desktop",
        ".hidden.desktop",
        "a b.desktop",
        "../x.desktop",
        "x/y.desktop",
        "x.desktop\n",
        "x.desktop.bak",
        "caf\u{e9}.desktop",
        "x;rm.desktop",
        long.as_str(),
    ] {
        assert!(!valid_desktop_id(bad), "{bad:?}");
    }
    assert!(valid_desktop_id(&format!("{}.desktop", "a".repeat(200))));
}

#[test]
fn empty_dpkg_and_rpm_on_arch_are_unavailable_not_failed() {
    // An Arch host with dpkg and rpm-tools installed from pacman: an
    // empty dpkg status, no info dir, an rpm binary and no rpm database.
    let t = FixtureTree::new();
    t.mkdir("/var/lib/pacman/local");
    t.write("/var/lib/dpkg/status", b"");
    t.write_exec("/usr/bin/rpm", b"#!/bin/sh\nexit 1\n");
    let inv = discover(
        &t.env(),
        &FakeRunner::new(),
        &ListOptions::default(),
        &Config::default(),
    );
    assert!(inv.errors.is_empty(), "{:?}", inv.errors);
    let st = |k| inv.statuses.iter().find(|s| s.source == k).unwrap();
    assert!(!st(SourceKind::Dpkg).available);
    assert!(!st(SourceKind::Rpm).available);
    assert!(inv.owned.is_complete());
    assert_eq!(
        crate::ownership::detect_package_db(&t.env()),
        Some(crate::ownership::PackageDb::Pacman)
    );
}

#[test]
fn inventory_serializes() {
    let t = FixtureTree::new();
    let inv = discover(
        &t.env(),
        &FakeRunner::new(),
        &ListOptions::default(),
        &Config::default(),
    );
    let v = serde_json::to_value(&inv).unwrap();
    assert_eq!(v["statuses"][6]["source"], "unmanaged");
    assert!(v["apps"].is_array());
}

#[test]
fn owned_index_completeness_follows_package_db() {
    let run = |t: &FixtureTree| {
        discover(
            &t.env(),
            &FakeRunner::new(),
            &ListOptions::default(),
            &Config::default(),
        )
    };
    let t = FixtureTree::new();
    t.pacman_pkg("ok", "1-1", Some(1), &["opt/ok/"]);
    let inv = run(&t);
    assert!(inv.owned.is_complete());
    assert!(inv.owned.owns(std::path::Path::new("/opt/ok")));

    // One package's `files` is unreadable (a directory).
    t.write("/var/lib/pacman/local/bad-1-1/desc", b"%NAME%\nbad\n\n");
    t.write("/var/lib/pacman/local/bad-1-1/files/x", b"");
    let inv = run(&t);
    assert!(!inv.owned.is_complete());
}
