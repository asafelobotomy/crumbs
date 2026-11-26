use super::*;

#[test]
fn exact_config_dir_is_high() {
    let t = FixtureTree::new();
    t.write("/home/user/.config/GutCheck/settings.json", b"{}");
    let r = Run::new(&t, vec![appimage("GutCheck-0.19.0-x86_64.AppImage")]);
    let fs = r.find(0, false);
    let f = must(&fs, ".config/GutCheck");
    assert_eq!(f.kind, FindingKind::Config);
    assert_eq!(f.confidence, Confidence::High);
    assert!(!f.shared);
    assert!(f.preselected);
    assert_eq!(f.size.files, 1);
    assert_eq!(f.fingerprint, fingerprint(&t.env(), &f.path).unwrap());
}

#[test]
fn normalized_match_is_medium() {
    let t = FixtureTree::new();
    t.mkdir("/home/user/.config/Grok Bot");
    let r = Run::new(&t, vec![appimage("Grok_Bot-1.0-x86_64.AppImage")]);
    let fs = r.find(0, false);
    let f = must(&fs, ".config/Grok Bot");
    assert_eq!(f.confidence, Confidence::Medium);
    assert!(!f.preselected);
}

#[test]
fn dot_dir_exact() {
    let t = FixtureTree::new();
    t.mkdir("/home/user/.hauler");
    t.mkdir("/home/user/.grokbot");
    let r = Run::new(
        &t,
        vec![
            appimage("hauler-x86_64.AppImage"),
            appimage("Grok_Bot-1.0-x86_64.AppImage"),
        ],
    );
    let hauler = r.find(0, false);
    let f = must(&hauler, ".hauler");
    assert_eq!(f.kind, FindingKind::DotDir);
    assert_eq!(f.confidence, Confidence::High);
    assert!(at(&hauler, ".grokbot").is_none());

    let grok = r.find(1, false);
    assert_eq!(must(&grok, ".grokbot").confidence, Confidence::Medium);
}

#[test]
fn dot_file_and_symlink_are_dot_file() {
    let t = FixtureTree::new();
    t.write("/home/user/.hauler", b"x");
    t.mkdir("/home/user/elsewhere");
    std::os::unix::fs::symlink(
        t.env().sys("/home/user/elsewhere"),
        t.env().sys("/home/user/.grokbot"),
    )
    .unwrap();
    let r = Run::new(
        &t,
        vec![
            appimage("hauler-x86_64.AppImage"),
            appimage("Grok_Bot-1.0-x86_64.AppImage"),
        ],
    );
    assert_eq!(
        must(&r.find(0, false), ".hauler").kind,
        FindingKind::DotFile
    );
    assert_eq!(
        must(&r.find(1, false), ".grokbot").kind,
        FindingKind::DotFile
    );
}

#[test]
fn rule_paths_high() {
    let t = FixtureTree::new();
    t.mkdir("/home/user/.mozilla/firefox/abc.default");
    let env = t.env();
    let (rules, errors) = load_rules(&env);
    assert!(errors.is_empty(), "{errors:?}");
    let r = Run::with(
        &t,
        vec![pacman("firefox", &["firefox"])],
        &rules,
        &Config::default(),
    );
    let fs = r.find(0, true);
    let f = must(&fs, ".mozilla/firefox");
    assert_eq!(f.confidence, Confidence::High);
    assert_eq!(f.kind, FindingKind::DotDir);
    assert!(f.preselected);
    assert!(f.reason.contains("rule"), "{}", f.reason);
    assert!(at(&fs, ".mozilla").is_none(), "{fs:#?}");
}

#[test]
fn rule_path_is_made_canonical() {
    let t = FixtureTree::new();
    t.mkdir("/home/user/.foo-data/x");
    let rules = [user_rule(&["foo"], &["~/.foo-data/", "~/./.foo-data/x"])];
    let r = Run::with(&t, vec![pacman("foo", &[])], &rules, &Config::default());
    let fs = r.find(0, false);
    let f = must(&fs, ".foo-data");
    assert_eq!(f.path, Path::new("/home/user/.foo-data"));
    assert_eq!(f.confidence, Confidence::High);
    // `~/./.foo-data/x` is normalised to `~/.foo-data/x`, which is nested
    // inside an equally confident finding and so not reported twice.
    assert_eq!(fs.len(), 1, "{fs:#?}");
}

#[test]
fn nested_finding_under_equal_parent_is_dropped() {
    let t = FixtureTree::new();
    t.mkdir("/home/user/.config/foo/profiles");
    let rules = [user_rule(&["foo"], &["$XDG_CONFIG_HOME/foo/profiles"])];
    let r = Run::with(&t, vec![pacman("foo", &[])], &rules, &Config::default());
    let fs = r.find(0, true);
    assert!(at(&fs, ".config/foo").is_some());
    assert!(at(&fs, ".config/foo/profiles").is_none(), "{fs:#?}");
}

#[test]
fn flatpak_var_app_high() {
    let t = FixtureTree::new();
    t.mkdir("/home/user/.var/app/org.mozilla.firefox/data");
    let r = Run::new(
        &t,
        vec![
            flatpak("org.mozilla.firefox"),
            pacman("firefox", &["firefox"]),
        ],
    );
    let fs = r.find(0, false);
    let f = must(&fs, ".var/app/org.mozilla.firefox");
    assert_eq!(f.kind, FindingKind::FlatpakData);
    assert_eq!(f.confidence, Confidence::High);
    assert!(f.preselected);
    // Never matched to anything but the Flatpak with that ID.
    assert!(r.find(1, true).is_empty(), "{:#?}", r.find(1, true));
}

#[test]
fn flatpak_matches_outside_own_data_capped_at_medium() {
    let t = FixtureTree::new();
    let id = "com.valvesoftware.Steam";
    t.mkdir("/home/user/.local/share/Steam");
    t.mkdir("/home/user/.steam");
    t.mkdir(&format!("/home/user/.var/app/{id}"));
    t.desktop(
        &format!("/home/user/.local/share/applications/{id}.desktop"),
        "[Desktop Entry]\nType=Application\nExec=flatpak run com.valvesoftware.Steam\nX-Flatpak=com.valvesoftware.Steam\n",
    );
    let rule = Rule {
        flatpaks: vec![id.into()],
        packages: Vec::new(),
        ..user_rule(&[], &["~/.steam"])
    };
    let r = Run::with(&t, vec![flatpak(id)], &[rule], &Config::default());
    let fs = r.find(0, false);
    let steam = must(&fs, ".local/share/Steam");
    assert_eq!(steam.confidence, Confidence::Medium);
    assert!(!steam.preselected);
    assert_eq!(must(&fs, ".steam").confidence, Confidence::Medium);
    assert_eq!(
        must(&fs, &format!(".var/app/{id}")).confidence,
        Confidence::High
    );
    let launcher = must(&fs, &format!(".local/share/applications/{id}.desktop"));
    assert_eq!(launcher.confidence, Confidence::High);
}

#[test]
fn flatpak_icons_need_full_id_or_desktop_stem() {
    let t = FixtureTree::new();
    t.write(
        "/home/user/.local/share/icons/hicolor/48x48/apps/firefox.png",
        b"p",
    );
    t.write(
        "/home/user/.local/share/icons/hicolor/48x48/apps/org.mozilla.firefox.png",
        b"p",
    );
    let r = Run::new(&t, vec![flatpak("org.mozilla.firefox")]);
    let fs = r.find(0, true);
    assert!(
        at(&fs, ".local/share/icons/hicolor/48x48/apps/firefox.png").is_none(),
        "{fs:#?}"
    );
    let own = must(
        &fs,
        ".local/share/icons/hicolor/48x48/apps/org.mozilla.firefox.png",
    );
    assert_eq!(own.confidence, Confidence::High);

    let mut with_stem = flatpak("org.mozilla.firefox");
    with_stem.desktop_ids.push("firefox.desktop".into());
    let r = Run::new(&t, vec![with_stem]);
    let fs = r.find(0, true);
    assert!(at(&fs, ".local/share/icons/hicolor/48x48/apps/firefox.png").is_some());
}

#[test]
fn flatpak_var_app_without_install_is_medium() {
    let t = FixtureTree::new();
    t.mkdir("/home/user/.var/app/org.gone.App");
    t.mkdir("/home/user/.var/app/org.here.App");
    let fs = orphans(&t, vec![flatpak("org.here.App")], false);
    let f = must(&fs, ".var/app/org.gone.App");
    assert_eq!(f.confidence, Confidence::Medium);
    assert_eq!(f.kind, FindingKind::FlatpakData);
    assert!(at(&fs, ".var/app/org.here.App").is_none());
}

#[test]
fn autostart_and_local_desktop_entry() {
    let t = FixtureTree::new();
    t.desktop(
        "/home/user/.config/autostart/foo.desktop",
        "[Desktop Entry]\nType=Application\nExec=foo-bin --tray\n",
    );
    t.desktop(
        "/home/user/.config/autostart/helper.desktop",
        "[Desktop Entry]\nType=Application\nExec=/usr/bin/foo-bin --minimized\n",
    );
    t.desktop(
        "/home/user/.local/share/applications/foo.desktop",
        "[Desktop Entry]\nType=Application\nExec=foo-bin %U\n",
    );
    t.desktop(
        "/home/user/.local/share/applications/unrelated.desktop",
        "[Desktop Entry]\nType=Application\nExec=bar\n",
    );
    let mut foo = pacman("foo", &["foo-bin"]);
    foo.desktop_ids = vec!["foo.desktop".into()];
    let r = Run::new(&t, vec![foo]);
    let fs = r.find(0, true);

    let a = must(&fs, ".config/autostart/foo.desktop");
    assert_eq!(a.kind, FindingKind::Autostart);
    assert_eq!(a.confidence, Confidence::High);
    let h = must(&fs, ".config/autostart/helper.desktop");
    assert_eq!(h.kind, FindingKind::Autostart);
    assert_eq!(h.confidence, Confidence::Medium);
    let d = must(&fs, ".local/share/applications/foo.desktop");
    assert_eq!(d.kind, FindingKind::DesktopEntry);
    assert_eq!(d.confidence, Confidence::High);
    assert!(at(&fs, ".local/share/applications/unrelated.desktop").is_none());
    // The container directories themselves are never findings.
    assert!(at(&fs, ".config/autostart").is_none());
}

#[test]
fn autostart_name_matched() {
    let t = FixtureTree::new();
    t.desktop(
        "/home/user/.config/autostart/gutcheck.desktop",
        "[Desktop Entry]\nType=Application\nName=GutCheck\nExec=/opt/elsewhere/run --tray\n",
    );
    t.desktop(
        "/home/user/.config/autostart/unrelated.desktop",
        "[Desktop Entry]\nType=Application\nName=Other\nExec=other\n",
    );
    let r = Run::new(&t, vec![appimage("GutCheck-0.19.0-x86_64.AppImage")]);
    let fs = r.find(0, true);
    let f = must(&fs, ".config/autostart/gutcheck.desktop");
    assert_eq!(f.kind, FindingKind::Autostart);
    assert_eq!(f.confidence, Confidence::High);
    assert!(f.preselected);
    assert!(at(&fs, ".config/autostart/unrelated.desktop").is_none());

    // A stoplisted stem says nothing about an app.
    let t = FixtureTree::new();
    t.desktop(
        "/home/user/.config/autostart/settings.desktop",
        "[Desktop Entry]\nType=Application\nExec=zzz\n",
    );
    let r = Run::new(&t, vec![pacman("settings", &[])]);
    assert!(at(&r.find(0, true), ".config/autostart/settings.desktop").is_none());
}

#[test]
fn appimage_launcher_is_high_for_it_and_unmatched_for_others() {
    let t = FixtureTree::new();
    let file = "GutCheck-0.19.0-x86_64.AppImage";
    t.desktop(
        "/home/user/.local/share/applications/appimagekit-gutcheck.desktop",
        &format!("[Desktop Entry]\nType=Application\nExec=\"{HOME}/Applications/{file}\" %U\n"),
    );
    t.desktop(
        "/home/user/.local/share/applications/gutcheck-tray.desktop",
        &format!("[Desktop Entry]\nType=Application\nExec={HOME}/Applications/{file} --tray\n"),
    );
    let mut ai = appimage(file);
    ai.desktop_ids = vec!["appimagekit-gutcheck.desktop".into()];
    ai.exec_names = vec![file.into()];
    let other = pacman("gutcheck-cli", &[file]);
    let r = Run::new(&t, vec![ai, other]);

    let fs = r.find(0, false);
    let own = must(
        &fs,
        ".local/share/applications/appimagekit-gutcheck.desktop",
    );
    assert_eq!(own.confidence, Confidence::High);
    assert!(!own.shared && own.preselected);
    // Not in desktop_ids, but its Exec is the AppImage file itself.
    let tray = must(&fs, ".local/share/applications/gutcheck-tray.desktop");
    assert_eq!(tray.confidence, Confidence::Medium);
    assert!(
        tray.shared,
        "exec-name match by the other app makes it shared"
    );

    let others = r.find(1, true);
    assert!(
        at(
            &others,
            ".local/share/applications/appimagekit-gutcheck.desktop"
        )
        .is_none(),
        "{others:#?}"
    );
}
