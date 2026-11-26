use super::*;

#[test]
fn snap_data_never_matched_by_other_sources() {
    let t = FixtureTree::new();
    t.mkdir("/home/user/snap/firefox/common");
    let r = Run::new(&t, vec![pacman("firefox", &["firefox"])]);
    assert!(r.find(0, true).is_empty(), "{:#?}", r.find(0, true));
    let i = r
        .idx
        .candidates()
        .iter()
        .position(|c| c.path == Path::new("/home/user/snap/firefox"))
        .expect("snap dir is a candidate");
    assert_eq!(r.idx.candidates()[i].kind, FindingKind::SnapData);
    assert!(!r.idx.matched_by_any(i));
}

#[test]
fn uninstalled_rule_app_is_medium() {
    let t = FixtureTree::new();
    t.write("/home/user/.thunderbird/profiles.ini", b"x");
    set_age(&t, "/home/user/.thunderbird", 60);
    let fs = orphans_at(
        &t,
        vec![pacman("firefox", &["firefox"])],
        false,
        SystemTime::now(),
    );
    let f = must(&fs, ".thunderbird");
    assert_eq!(f.confidence, Confidence::Medium);
    assert_eq!(
        f.reason,
        "looks like thunderbird data; thunderbird was not detected as installed"
    );
    assert_eq!(f.kind, FindingKind::DotDir);
    assert_eq!(f.size.files, 1);
    assert_eq!(f.fingerprint, fingerprint(&t.env(), &f.path).unwrap());
    assert!(!f.shared && f.mime_lines.is_empty());
}

#[test]
fn recently_modified_rule_orphan_is_capped_low() {
    let t = FixtureTree::new();
    t.write("/home/user/.thunderbird/profiles.ini", b"x");
    let now = SystemTime::now();
    assert!(orphans_at(&t, Vec::new(), false, now).is_empty());
    let fs = orphans_at(&t, Vec::new(), true, now);
    let f = must(&fs, ".thunderbird");
    assert_eq!(f.confidence, Confidence::Low);
    assert_eq!(
        f.reason,
        "looks like thunderbird data; thunderbird was not detected as installed; modified 0 days ago"
    );
    // The same directory seen 5 days later.
    let fs = orphans_at(&t, Vec::new(), true, now + DAY * 5);
    assert!(
        must(&fs, ".thunderbird")
            .reason
            .ends_with("; modified 5 days ago")
    );
}

#[test]
fn recency_cap_applies_to_medium_flatpak_dir() {
    let t = FixtureTree::new();
    t.mkdir("/home/user/.var/app/org.gone.App");
    let fs = orphans_at(&t, Vec::new(), true, SystemTime::now());
    let f = must(&fs, ".var/app/org.gone.App");
    assert_eq!(f.confidence, Confidence::Low);
    assert_eq!(
        f.reason,
        "data of the Flatpak org.gone.App, which is not installed; modified 0 days ago"
    );
}

#[test]
fn brave_origin_bin_keeps_brave_config() {
    let t = FixtureTree::new();
    t.mkdir("/home/user/.config/BraveSoftware/Brave-Browser");
    t.mkdir("/home/user/.cache/BraveSoftware");
    let fs = orphans(&t, vec![pacman("brave-origin-bin", &["brave"])], true);
    assert!(at(&fs, ".config/BraveSoftware").is_none(), "{fs:#?}");
    assert!(at(&fs, ".cache/BraveSoftware").is_none(), "{fs:#?}");
}

#[test]
fn installed_app_dir_not_orphan() {
    let t = FixtureTree::new();
    t.mkdir("/home/user/.thunderbird");
    t.mkdir("/home/user/.config/GutCheck");
    let apps = vec![
        pacman("thunderbird", &["thunderbird"]),
        appimage("GutCheck-0.19.0-x86_64.AppImage"),
    ];
    let fs = orphans(&t, apps, true);
    assert!(fs.is_empty(), "{fs:#?}");
}

#[test]
fn unknown_dir_is_low_hidden_by_default() {
    let t = FixtureTree::new();
    t.mkdir("/home/user/.config/mystery-tool");
    t.mkdir("/home/user/snap/oldsnap");
    assert!(orphans(&t, Vec::new(), false).is_empty());
    let fs = orphans(&t, Vec::new(), true);
    assert_eq!(
        must(&fs, ".config/mystery-tool").confidence,
        Confidence::Low
    );
    let snap = must(&fs, "snap/oldsnap");
    assert_eq!(snap.confidence, Confidence::Low);
    assert_eq!(snap.kind, FindingKind::SnapData);
}

#[test]
fn broken_autostart_is_medium() {
    let t = FixtureTree::new();
    t.write_exec("/usr/bin/present", b"#!/bin/sh\n");
    t.desktop(
        "/home/user/.config/autostart/gone.desktop",
        "[Desktop Entry]\nType=Application\nName=Gone\nExec=/opt/gone/bin/gone --x\n",
    );
    t.desktop(
        "/home/user/.config/autostart/ok.desktop",
        "[Desktop Entry]\nType=Application\nName=Ok\nExec=present\n",
    );
    t.desktop(
        "/home/user/.local/share/applications/lost.desktop",
        "[Desktop Entry]\nType=Application\nName=Lost\nExec=lostbin %U\n",
    );
    let fs = orphans(&t, Vec::new(), false);
    let gone = must(&fs, ".config/autostart/gone.desktop");
    assert_eq!(gone.confidence, Confidence::Medium);
    assert_eq!(gone.kind, FindingKind::Autostart);
    let lost = must(&fs, ".local/share/applications/lost.desktop");
    assert_eq!(lost.confidence, Confidence::Medium);
    assert_eq!(lost.kind, FindingKind::DesktopEntry);
    assert!(at(&fs, ".config/autostart/ok.desktop").is_none());
    let all = orphans(&t, Vec::new(), true);
    assert_eq!(
        must(&all, ".config/autostart/ok.desktop").confidence,
        Confidence::Low
    );
}

#[test]
fn desktop_shell_files_ignored() {
    let t = FixtureTree::new();
    t.write("/home/user/.bashrc", b"x");
    t.write("/home/user/.config/kwinrc", b"x");
    t.mkdir("/home/user/.config/gtk-3.0");
    t.mkdir("/home/user/.config/plasma-workspace");
    t.mkdir("/home/user/.config/plasmashell");
    t.write("/home/user/.local/share/kwinrc", b"x");
    let fs = orphans(&t, Vec::new(), true);
    assert!(at(&fs, ".bashrc").is_none());
    assert!(at(&fs, ".config/kwinrc").is_none());
    assert!(at(&fs, ".config/gtk-3.0").is_none());
    assert!(at(&fs, ".config/plasma-workspace").is_none());
    assert!(at(&fs, ".config/plasmashell").is_none());
    // The `rc` rule is for the config directory only.
    assert!(at(&fs, ".local/share/kwinrc").is_some(), "{fs:#?}");
}

#[test]
fn orphans_never_preselected() {
    let t = FixtureTree::new();
    t.mkdir("/home/user/.thunderbird");
    t.mkdir("/home/user/.var/app/org.gone.App");
    t.mkdir("/home/user/.config/mystery-tool");
    let fs = orphans(&t, Vec::new(), true);
    assert_eq!(fs.len(), 3, "{fs:#?}");
    assert!(fs.iter().all(|f| !f.preselected));
    assert!(fs.iter().all(|f| f.confidence != Confidence::High));
    let order: Vec<_> = fs.iter().map(|f| f.confidence).collect();
    assert_eq!(
        order,
        [Confidence::Medium, Confidence::Medium, Confidence::Low]
    );
}

#[test]
fn orphans_no_longer_list_named_autostart() {
    let t = FixtureTree::new();
    t.desktop(
        "/home/user/.config/autostart/gutcheck.desktop",
        "[Desktop Entry]\nType=Application\nName=GutCheck\nExec=/opt/elsewhere/run\n",
    );
    let apps = vec![appimage("GutCheck-0.19.0-x86_64.AppImage")];
    let fs = orphans(&t, apps, true);
    assert!(
        at(&fs, ".config/autostart/gutcheck.desktop").is_none(),
        "{fs:#?}"
    );
    // Without the app it is an orphan again.
    let fs = orphans(&t, Vec::new(), true);
    must(&fs, ".config/autostart/gutcheck.desktop");
}

#[test]
fn regression_parent_of_live_rule_path_not_orphan() {
    let t = FixtureTree::new();
    t.mkdir("/home/user/.mozilla/firefox/profile");
    let fs = orphans(&t, vec![pacman("firefox", &["firefox"])], true);
    assert!(at(&fs, ".mozilla").is_none(), "{fs:#?}");
}

#[test]
fn regression_equal_live_rule_path_not_orphan() {
    let t = FixtureTree::new();
    t.mkdir("/home/user/.config/BraveSoftware/Brave-Browser");
    let fs = orphans(&t, vec![pacman("brave-bin", &["brave"])], true);
    assert!(at(&fs, ".config/BraveSoftware").is_none(), "{fs:#?}");
}

#[test]
fn regression_path_live_for_one_rule_gone_for_another_not_orphan() {
    let t = FixtureTree::new();
    t.mkdir("/home/user/.vscode-oss/extensions");
    let fs = orphans(&t, vec![pacman("vscodium", &["codium"])], true);
    assert!(at(&fs, ".vscode-oss").is_none(), "{fs:#?}");
}

/// `$XDG_CONFIG_HOME/crumbs` and `$XDG_STATE_HOME/crumbs` outside the
/// default locations are crumbs' own: never a finding or an orphan.
#[test]
fn custom_xdg_crumbs_dirs_never_found() {
    let t = FixtureTree::new();
    let mut env = t.env();
    env.config_home = env.home.join("xdg/config");
    env.state_home = env.home.join("xdg/state");
    t.write("/home/user/xdg/config/crumbs/config.toml", b"");
    t.write("/home/user/xdg/state/crumbs/history.jsonl", b"");
    set_age(&t, "/home/user/xdg/config/crumbs", 60);
    set_age(&t, "/home/user/xdg/state/crumbs", 60);
    let rules = [user_rule(
        &["crumbs"],
        &["$XDG_CONFIG_HOME/crumbs", "$XDG_STATE_HOME/crumbs"],
    )];
    let with_app = inv(vec![pacman("crumbs", &["crumbs"])]);
    let idx = LeftoverIndex::build(&env, &Config::default(), &rules, &with_app);
    let fs = find_leftovers(&idx, &with_app.apps[0], true);
    assert!(fs.is_empty(), "{fs:#?}");
    let idx = LeftoverIndex::build(&env, &Config::default(), &[], &inv(vec![]));
    let orphans = super::super::orphans::find_orphans_at(&idx, true, SystemTime::now() + DAY * 60);
    assert!(
        !orphans.iter().any(|f| f.path.ends_with("crumbs")),
        "{orphans:#?}"
    );
}
