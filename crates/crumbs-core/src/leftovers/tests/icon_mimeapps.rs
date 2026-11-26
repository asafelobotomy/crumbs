use super::*;

#[test]
fn icon_by_desktop_id() {
    let t = FixtureTree::new();
    t.write(
        "/home/user/.local/share/icons/hicolor/48x48/apps/foo.png",
        b"p",
    );
    t.write("/home/user/.local/share/icons/foo.svg", b"s");
    t.write(
        "/home/user/.local/share/icons/hicolor/scalable/apps/org.example.Bar.svg",
        b"s",
    );
    // A third-party icon theme's copy belongs to the theme.
    t.write(
        "/home/user/.local/share/icons/Papirus/48x48/apps/foo.svg",
        b"s",
    );
    let mut foo = pacman("foo", &[]);
    foo.desktop_ids = vec!["foo.desktop".into()];
    let r = Run::new(&t, vec![foo, flatpak("org.example.Bar")]);

    let fs = r.find(0, true);
    let i = must(&fs, ".local/share/icons/hicolor/48x48/apps/foo.png");
    assert_eq!(i.kind, FindingKind::Icon);
    assert_eq!(i.confidence, Confidence::High);
    assert!(at(&fs, ".local/share/icons/foo.svg").is_some());
    assert!(at(&fs, ".local/share/icons/Papirus/48x48/apps/foo.svg").is_none());
    assert!(at(&fs, ".local/share/icons").is_none());

    let bar = r.find(1, false);
    let b = must(
        &bar,
        ".local/share/icons/hicolor/scalable/apps/org.example.Bar.svg",
    );
    assert_eq!(b.confidence, Confidence::High);
}

#[test]
fn mimeapps_lines_collected() {
    let t = FixtureTree::new();
    t.write(
        "/home/user/.config/mimeapps.list",
        b"[Default Applications]\ntext/plain=gutcheck.desktop\nimage/png=other.desktop\n\n[Added Associations]\nx-scheme-handler/gc=gutcheck.desktop;\n",
    );
    let mut gc = pacman("gutcheck", &[]);
    gc.desktop_ids = vec!["gutcheck.desktop".into()];
    let r = Run::new(&t, vec![gc]);
    let fs = r.find(0, false);
    let f = must(&fs, ".config/mimeapps.list");
    assert_eq!(f.kind, FindingKind::MimeAssoc);
    assert_eq!(f.confidence, Confidence::High);
    assert!(f.preselected);
    assert_eq!(f.mime_lines.len(), 2);
    assert_eq!(
        f.mime_lines,
        [
            "text/plain=gutcheck.desktop",
            "x-scheme-handler/gc=gutcheck.desktop;"
        ]
    );
    assert_eq!(fs.len(), 1, "{fs:#?}");
}

#[test]
fn mimeapps_line_naming_another_app_is_shared() {
    let t = FixtureTree::new();
    t.write(
        "/home/user/.config/mimeapps.list",
        b"[Added Associations]\ntext/html=firefox.desktop;chromium.desktop;\n",
    );
    let mut ff = pacman("firefox", &[]);
    ff.desktop_ids = vec!["firefox.desktop".into()];
    let mut ch = pacman("chromium", &[]);
    ch.desktop_ids = vec!["chromium.desktop".into()];
    let r = Run::new(&t, vec![ff, ch]);
    let fs = r.find(0, false);
    let f = must(&fs, ".config/mimeapps.list");
    assert_eq!(f.confidence, Confidence::Medium);
    assert!(f.shared && !f.preselected);
}

#[test]
fn systemd_user_unit_medium() {
    let t = FixtureTree::new();
    t.write(
        "/home/user/.config/systemd/user/sync.service",
        b"[Unit]\nDescription=x\n\n[Service]\nExecStart=-/usr/bin/syncthing serve --no-browser\n",
    );
    t.write(
        "/home/user/.config/systemd/user/other.service",
        b"[Service]\nExecStart=/usr/bin/other\n",
    );
    let r = Run::new(&t, vec![pacman("syncthing", &["syncthing"])]);
    let fs = r.find(0, true);
    let f = must(&fs, ".config/systemd/user/sync.service");
    assert_eq!(f.kind, FindingKind::SystemdUnit);
    assert_eq!(f.confidence, Confidence::Medium);
    assert!(at(&fs, ".config/systemd/user/other.service").is_none());
    assert!(at(&fs, ".config/systemd").is_none());
}

#[test]
fn shared_dir_downgraded() {
    let t = FixtureTree::new();
    t.mkdir("/home/user/.config/chromium");
    let r = Run::new(
        &t,
        vec![
            pacman("chromium", &["chromium"]),
            pacman("chromium-widevine", &["chromium"]),
        ],
    );
    let fs = r.find(0, false);
    let f = must(&fs, ".config/chromium");
    assert_eq!(f.confidence, Confidence::Medium);
    assert!(f.shared);
    assert!(!f.preselected);
    assert!(f.reason.contains("chromium-widevine"), "{}", f.reason);

    // Medium for widevine, downgraded to Low: hidden unless show_low.
    assert!(at(&r.find(1, false), ".config/chromium").is_none());
    let w = r.find(1, true);
    let f = must(&w, ".config/chromium");
    assert_eq!(f.confidence, Confidence::Low);
    assert!(f.shared && !f.preselected);
}

#[test]
fn shared_desktop_files_are_not_orphans() {
    let t = FixtureTree::new();
    t.write("/home/user/.config/kdeglobals", b"[General]");
    t.write("/home/user/.local/share/recently-used.xbel", b"<xbel/>");
    t.write("/home/user/.local/share/user-places.xbel", b"<xbel/>");
    t.mkdir("/home/user/.local/share/pki/nssdb");
    t.mkdir("/home/user/.local/share/containers/storage");
    let fs = orphans(&t, Vec::new(), true);
    assert!(fs.is_empty(), "{fs:#?}");
}

#[test]
fn short_and_generic_names_ignored() {
    let t = FixtureTree::new();
    t.mkdir("/home/user/.config/go");
    t.mkdir("/home/user/.cache/code-insiders");
    let r = Run::new(&t, vec![pacman("go", &["code"])]);
    let fs = r.find(0, true);
    assert!(fs.is_empty(), "{fs:#?}");
}

#[test]
fn low_hidden_unless_show_low() {
    let t = FixtureTree::new();
    t.mkdir("/home/user/.cache/hauler-updater");
    let r = Run::new(&t, vec![appimage("hauler-x86_64.AppImage")]);
    assert!(r.find(0, false).is_empty());
    let fs = r.find(0, true);
    let f = must(&fs, ".cache/hauler-updater");
    assert_eq!(f.confidence, Confidence::Low);
    assert_eq!(f.kind, FindingKind::Cache);
    assert!(!f.preselected);
}

#[test]
fn protected_never_found() {
    let t = FixtureTree::new();
    t.mkdir("/home/user/.gnupg");
    t.mkdir("/home/user/.config/autostart");
    t.mkdir("/home/user/Documents");
    let rules = [user_rule(
        &["gnupg"],
        &["~/.gnupg", "~/Documents", "$XDG_CONFIG_HOME/autostart"],
    )];
    let r = Run::with(&t, vec![pacman("gnupg", &[])], &rules, &Config::default());
    let fs = r.find(0, true);
    assert!(fs.is_empty(), "{fs:#?}");
}

#[test]
fn excluded_never_found() {
    let t = FixtureTree::new();
    t.mkdir("/home/user/.config/foo");
    t.mkdir("/home/user/.foo");
    let cfg = Config {
        exclude: vec!["/home/user/.config/foo".into(), "/home/user/.fo*".into()],
        ..Config::default()
    };
    let rules = [user_rule(&["foo"], &["~/.foo"])];
    let r = Run::with(&t, vec![pacman("foo", &[])], &rules, &cfg);
    let fs = r.find(0, true);
    assert!(fs.is_empty(), "{fs:#?}");
}

#[test]
fn version_suffix_stripped() {
    let t = FixtureTree::new();
    t.mkdir("/home/user/.config/Foo-2");
    t.mkdir("/home/user/.local/share/foo_3");
    // Only one final segment is dropped: `foo-1.2` → `foo-1`, no match.
    t.mkdir("/home/user/.cache/foo-1.2");
    let r = Run::new(&t, vec![pacman("foo", &[])]);
    let fs = r.find(0, true);
    assert_eq!(must(&fs, ".config/Foo-2").confidence, Confidence::Medium);
    let d = must(&fs, ".local/share/foo_3");
    assert_eq!(d.confidence, Confidence::Medium);
    assert_eq!(d.kind, FindingKind::Data);
    assert!(at(&fs, ".cache/foo-1.2").is_none(), "{fs:#?}");
}

#[test]
fn results_sorted_by_confidence_then_path() {
    let t = FixtureTree::new();
    t.mkdir("/home/user/.config/kite");
    t.mkdir("/home/user/.cache/kite");
    t.mkdir("/home/user/.local/state/Kite");
    t.mkdir("/home/user/.cache/kiteboard-x");
    let r = Run::new(&t, vec![pacman("kite", &[])]);
    let got: Vec<(Confidence, String)> = r
        .find(0, true)
        .iter()
        .map(|f| (f.confidence, f.path.display().to_string()))
        .collect();
    let want = [
        (Confidence::High, "/home/user/.cache/kite"),
        (Confidence::High, "/home/user/.config/kite"),
        (Confidence::Medium, "/home/user/.local/state/Kite"),
        (Confidence::Low, "/home/user/.cache/kiteboard-x"),
    ];
    let want: Vec<(Confidence, String)> = want.iter().map(|(c, p)| (*c, p.to_string())).collect();
    assert_eq!(got, want);
}

#[test]
fn index_exposes_candidates_and_match_state() {
    let t = FixtureTree::new();
    t.mkdir("/home/user/.config/kite");
    t.mkdir("/home/user/.config/nobody-owns-this");
    t.mkdir("/home/user/.cache/kite-helper");
    let r = Run::new(&t, vec![pacman("kite", &[])]);
    let idx = &r.idx;
    let state = |p: &str| {
        let i = idx
            .candidates()
            .iter()
            .position(|c| c.path == Path::new(p))
            .unwrap_or_else(|| panic!("{p} not a candidate"));
        idx.matched_by_any(i)
    };
    assert!(state("/home/user/.config/kite"));
    assert!(
        state("/home/user/.cache/kite-helper"),
        "Low counts as matched"
    );
    assert!(!state("/home/user/.config/nobody-owns-this"));
    assert_eq!(idx.inventory().apps.len(), 1);
    assert!(idx.rules().is_empty());
}

#[test]
fn serializes_kebab_and_lowercase_without_fingerprint() {
    let t = FixtureTree::new();
    t.mkdir("/home/user/.var/app/org.example.Bar");
    let r = Run::new(&t, vec![flatpak("org.example.Bar")]);
    let fs = r.find(0, false);
    let v = serde_json::to_value(&fs[0]).unwrap();
    assert_eq!(v["kind"], "flatpak-data");
    assert_eq!(v["confidence"], "high");
    assert!(v.get("fingerprint").is_none());
    assert!(Confidence::Low < Confidence::Medium && Confidence::Medium < Confidence::High);
}

#[test]
fn other_apps_data_inside_makes_parent_shared() {
    let t = FixtureTree::new();
    t.mkdir("/home/user/.config/foo/b-plugin");
    t.mkdir("/home/user/.config/foo/both");
    let rules = [
        user_rule(
            &["bar"],
            &["$XDG_CONFIG_HOME/foo/b-plugin", "$XDG_CONFIG_HOME/foo/both"],
        ),
        user_rule(&["foo"], &["$XDG_CONFIG_HOME/foo/both"]),
    ];
    let r = Run::with(
        &t,
        vec![pacman("foo", &[]), pacman("bar", &[])],
        &rules,
        &Config::default(),
    );
    let fs = r.find(0, false);
    let parent = must(&fs, ".config/foo");
    assert!(parent.shared, "{parent:#?}");
    assert!(!parent.preselected);
    assert_eq!(parent.confidence, Confidence::Medium);
    assert!(parent.reason.contains("bar"), "{}", parent.reason);
    // A shared child stays listed even inside a finding of equal confidence.
    let both = must(&fs, ".config/foo/both");
    assert!(both.shared && !both.preselected);

    let bar = r.find(1, false);
    let plugin = must(&bar, ".config/foo/b-plugin");
    assert_eq!(plugin.confidence, Confidence::High);
    assert!(plugin.preselected);
}

#[test]
fn snap_data_matched_by_its_snap_only() {
    let t = FixtureTree::new();
    t.mkdir("/home/user/snap/firefox/common");
    t.mkdir("/home/user/snap/other/common");
    let mut snap = App::new(SourceKind::Snap, "firefox");
    snap.exec_names = vec!["firefox".into()];
    let r = Run::new(&t, vec![snap.clone()]);
    let fs = r.find(0, true);
    let f = must(&fs, "snap/firefox");
    assert_eq!(f.kind, FindingKind::SnapData);
    assert_eq!(f.confidence, Confidence::High);
    assert!(at(&fs, "snap/other").is_none(), "{fs:#?}");
    // An installed snap's data is never an orphan.
    let os = orphans(&t, vec![snap], true);
    assert!(at(&os, "snap/firefox").is_none(), "{os:#?}");
    assert!(at(&os, "snap/other").is_some(), "{os:#?}");
}
