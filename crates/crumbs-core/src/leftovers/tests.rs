#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use super::*;
use crate::config::Config;
use crate::inventory::Inventory;
use crate::model::{App, Installation, SourceKind};
use crate::rules::{Rule, RuleOrigin, load_rules};
use crate::safety::fingerprint;
use crate::testing::FixtureTree;

const HOME: &str = "/home/user";

fn inv(apps: Vec<App>) -> Inventory {
    Inventory {
        apps,
        statuses: Vec::new(),
        errors: Vec::new(),
        owned: Default::default(),
    }
}

fn appimage(file: &str) -> App {
    let (base, version) = crate::sources::appimage::split_appimage_name(file);
    let mut a = App::new(SourceKind::AppImage, file);
    a.id = format!("appimage:~/Applications/{file}");
    a.display_name = base;
    a.version = version;
    a.installation = Installation::User;
    a.location = Some(PathBuf::from(format!("{HOME}/Applications/{file}")));
    a
}

fn pacman(name: &str, exec: &[&str]) -> App {
    let mut a = App::new(SourceKind::Pacman, name);
    a.exec_names = exec.iter().map(|s| s.to_string()).collect();
    a
}

fn flatpak(id: &str) -> App {
    let mut a = App::new(SourceKind::Flatpak, id);
    a.id = format!("flatpak:user/{id}");
    a.rdns_id = Some(id.to_string());
    a.installation = Installation::User;
    a.desktop_ids = vec![format!("{id}.desktop")];
    a
}

fn user_rule(packages: &[&str], paths: &[&str]) -> Rule {
    Rule {
        packages: packages.iter().map(|s| s.to_string()).collect(),
        flatpaks: Vec::new(),
        appimages: Vec::new(),
        desktops: Vec::new(),
        paths: paths.iter().map(|s| s.to_string()).collect(),
        origin: RuleOrigin::User(PathBuf::from("/home/user/.config/crumbs/rules.d/t.toml")),
    }
}

struct Run {
    idx: LeftoverIndex,
    inv: Inventory,
}

impl Run {
    fn new(t: &FixtureTree, apps: Vec<App>) -> Run {
        Run::with(t, apps, &[], &Config::default())
    }

    fn with(t: &FixtureTree, apps: Vec<App>, rules: &[Rule], cfg: &Config) -> Run {
        let inv = inv(apps);
        let idx = LeftoverIndex::build(&t.env(), cfg, rules, &inv);
        Run { idx, inv }
    }

    fn find(&self, i: usize, show_low: bool) -> Vec<Finding> {
        find_leftovers(&self.idx, &self.inv.apps[i], show_low)
    }
}

fn at<'a>(fs: &'a [Finding], rel: &str) -> Option<&'a Finding> {
    let p = Path::new(HOME).join(rel);
    fs.iter().find(|f| f.path == p)
}

fn must<'a>(fs: &'a [Finding], rel: &str) -> &'a Finding {
    at(fs, rel).unwrap_or_else(|| panic!("no finding for ~/{rel}: {fs:#?}"))
}

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
fn snap_data_never_matched() {
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

const DAY: Duration = Duration::from_secs(86_400);

fn orphans_at(t: &FixtureTree, apps: Vec<App>, show_low: bool, now: SystemTime) -> Vec<Finding> {
    let (rules, _) = load_rules(&t.env());
    let r = Run::with(t, apps, &rules, &Config::default());
    super::orphans::find_orphans_at(&r.idx, show_low, now)
}

/// Orphans with the clock 60 days ahead, so fixture files count as old.
fn orphans(t: &FixtureTree, apps: Vec<App>, show_low: bool) -> Vec<Finding> {
    orphans_at(t, apps, show_low, SystemTime::now() + DAY * 60)
}

fn set_age(t: &FixtureTree, rel: &str, days: u64) {
    let f = std::fs::File::open(t.mkdir(rel)).unwrap();
    f.set_modified(SystemTime::now() - DAY * days as u32)
        .unwrap();
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
    let orphans = super::orphans::find_orphans_at(&idx, true, SystemTime::now() + DAY * 60);
    assert!(
        !orphans.iter().any(|f| f.path.ends_with("crumbs")),
        "{orphans:#?}"
    );
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

// ---- system leftovers (/etc, /opt) ----

mod system {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    use super::*;
    use crate::env::Env;
    use crate::leftovers::find_system_leftovers;
    use crate::ownership::OwnedIndex;
    use crate::safety::Scope;

    /// A fake root whose `/`, `/etc` and `/opt` pass the system-tree chain
    /// check, with this user standing in for root.
    fn sys_env(t: &FixtureTree) -> Env {
        t.mkdir("etc");
        t.mkdir("opt");
        for rel in ["", "etc", "opt"] {
            fs::set_permissions(t.dir.path().join(rel), fs::Permissions::from_mode(0o755)).unwrap();
        }
        let mut env = t.env();
        env.set_system_owner_uid(env.uid);
        env
    }

    fn owned(files: &[&str]) -> OwnedIndex {
        OwnedIndex::new(files.iter().map(PathBuf::from), Vec::<PathBuf>::new(), true)
    }

    struct Sys {
        env: Env,
        idx: LeftoverIndex,
        inv: Inventory,
    }

    fn sys(t: &FixtureTree, apps: Vec<App>, own: OwnedIndex, rules: &[Rule]) -> Sys {
        let env = sys_env(t);
        let mut i = inv(apps);
        i.owned = own;
        let idx = LeftoverIndex::build(&env, &Config::default(), rules, &i);
        Sys { env, idx, inv: i }
    }

    impl Sys {
        fn find(&self, i: usize, show_low: bool) -> Vec<Finding> {
            find_system_leftovers(
                &self.env,
                &Config::default(),
                &self.idx,
                &self.inv.apps[i],
                show_low,
            )
        }
    }

    fn sys_at<'a>(fs: &'a [Finding], path: &str) -> Option<&'a Finding> {
        fs.iter().find(|f| f.path == Path::new(path))
    }

    #[test]
    fn system_leftover_exact_name_is_medium_not_preselected() {
        let t = FixtureTree::new();
        t.write("opt/inky/bin", b"x");
        t.write("etc/inky/conf", b"x");
        let s = sys(
            &t,
            vec![pacman("inky", &[])],
            owned(&["/usr/bin/true"]),
            &[],
        );
        let fs = s.find(0, false);
        assert_eq!(fs.len(), 2, "{fs:#?}");
        for (path, kind) in [
            ("/opt/inky", FindingKind::SystemData),
            ("/etc/inky", FindingKind::SystemConfig),
        ] {
            let f = sys_at(&fs, path).unwrap_or_else(|| panic!("{path}: {fs:#?}"));
            assert_eq!(f.kind, kind);
            assert_eq!(f.confidence, Confidence::Medium);
            assert!(!f.preselected);
            assert!(!f.shared);
            assert_eq!(f.scope, Scope::SystemTree);
            assert_eq!(f.size.files, 1);
            assert_eq!(f.fingerprint, fingerprint(&s.env, &f.path).unwrap());
        }
    }

    #[test]
    fn owned_system_dir_not_offered() {
        let t = FixtureTree::new();
        t.write("opt/foo/file", b"x");
        let s = sys(&t, vec![pacman("foo", &[])], owned(&["/opt/foo/file"]), &[]);
        assert!(s.find(0, true).is_empty());
    }

    #[test]
    fn no_or_incomplete_package_database_offers_nothing() {
        let t = FixtureTree::new();
        t.mkdir("opt/inky");
        let s = sys(&t, vec![pacman("inky", &[])], OwnedIndex::default(), &[]);
        assert!(s.find(0, true).is_empty());
        let partial = OwnedIndex::new(
            [PathBuf::from("/usr/bin/true")],
            Vec::<PathBuf>::new(),
            false,
        );
        let s = sys(&t, vec![pacman("inky", &[])], partial, &[]);
        assert!(s.find(0, true).is_empty());
    }

    #[test]
    fn normalised_match_is_low_and_hidden() {
        let t = FixtureTree::new();
        t.mkdir("opt/Inky_App");
        let s = sys(
            &t,
            vec![pacman("inky-app", &[])],
            owned(&["/usr/bin/true"]),
            &[],
        );
        assert!(s.find(0, false).is_empty());
        let fs = s.find(0, true);
        assert_eq!(fs.len(), 1, "{fs:#?}");
        assert_eq!(fs[0].confidence, Confidence::Low);
        assert!(!fs[0].preselected);
    }

    #[test]
    fn substring_names_are_not_matched() {
        let t = FixtureTree::new();
        t.mkdir("opt/Inky-App");
        let s = sys(
            &t,
            vec![pacman("inky", &[])],
            owned(&["/usr/bin/true"]),
            &[],
        );
        assert!(s.find(0, true).is_empty());
    }

    #[test]
    fn etc_never_names_not_offered() {
        let t = FixtureTree::new();
        t.mkdir("etc/ssh");
        let s = sys(&t, vec![pacman("ssh", &[])], owned(&["/usr/bin/true"]), &[]);
        assert!(s.find(0, true).is_empty());
    }

    #[test]
    fn system_leftover_of_installed_app_is_shared_and_not_preselected() {
        let t = FixtureTree::new();
        t.mkdir("etc/brave");
        let s = sys(
            &t,
            vec![
                pacman("brave", &["brave"]),
                pacman("brave-beta", &["brave"]),
            ],
            owned(&["/usr/bin/true"]),
            &[],
        );
        let fs = s.find(0, true);
        assert_eq!(fs.len(), 1, "{fs:#?}");
        assert!(fs[0].shared);
        assert_eq!(fs[0].confidence, Confidence::Low);
        assert!(!fs[0].preselected);
        assert!(fs[0].reason.contains("shared with"), "{}", fs[0].reason);
        assert!(s.find(0, false).is_empty());
    }

    #[test]
    fn unreadable_system_leftover_listed_with_unknown_size() {
        struct Restore(PathBuf);
        impl Drop for Restore {
            fn drop(&mut self) {
                let _ = fs::set_permissions(&self.0, fs::Permissions::from_mode(0o755));
            }
        }
        let t = FixtureTree::new();
        t.write("etc/proton/a", b"x");
        let dir = t.dir.path().join("etc/proton");
        let _guard = Restore(dir.clone());
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o000)).unwrap();
        let s = sys(
            &t,
            vec![pacman("proton", &[])],
            owned(&["/usr/bin/true"]),
            &[],
        );
        let fs = s.find(0, false);
        let f = sys_at(&fs, "/etc/proton").unwrap_or_else(|| panic!("{fs:#?}"));
        assert!(f.size.errors > 0, "{:?}", f.size);
    }

    #[test]
    fn symlink_entry_is_not_offered() {
        let t = FixtureTree::new();
        t.mkdir("srv/real");
        t.symlink("opt/inky", "/srv/real");
        let s = sys(
            &t,
            vec![pacman("inky", &[])],
            owned(&["/usr/bin/true"]),
            &[],
        );
        assert!(s.find(0, true).is_empty());
    }

    #[test]
    fn writable_opt_offers_nothing() {
        let t = FixtureTree::new();
        t.mkdir("opt/inky");
        let s = sys(
            &t,
            vec![pacman("inky", &[])],
            owned(&["/usr/bin/true"]),
            &[],
        );
        fs::set_permissions(t.dir.path().join("opt"), fs::Permissions::from_mode(0o777)).unwrap();
        assert!(s.find(0, true).is_empty());
    }

    #[test]
    fn rule_may_name_etc_and_opt_children() {
        let t = FixtureTree::new();
        t.mkdir("etc/foo");
        t.mkdir("opt/other");
        t.write("etc/foo/bar", b"x");
        let rules = [user_rule(
            &["acme"],
            &["/etc/foo", "/etc/foo/bar", "/opt/other", "/usr/bin/x"],
        )];
        let s = sys(
            &t,
            vec![pacman("acme", &[])],
            owned(&["/usr/bin/true"]),
            &rules,
        );
        let fs = s.find(0, false);
        let paths: Vec<&Path> = fs.iter().map(|f| f.path.as_path()).collect();
        assert_eq!(paths, [Path::new("/etc/foo"), Path::new("/opt/other")]);
        assert!(fs.iter().all(|f| f.confidence == Confidence::Medium));
        assert!(fs[0].reason.contains("rule"), "{}", fs[0].reason);
    }

    #[test]
    fn tmpfiles_declared_directory_is_not_offered() {
        let t = FixtureTree::new();
        t.mkdir("etc/openvpn");
        t.write(
            "usr/lib/tmpfiles.d/openvpn.conf",
            b"d /etc/openvpn/client 0750 openvpn network -\n",
        );
        let s = sys(
            &t,
            vec![pacman("openvpn", &[])],
            owned(&["/usr/bin/true"]),
            &[],
        );
        assert!(s.find(0, true).is_empty());
    }

    #[test]
    fn exec_basename_alone_is_low() {
        let t = FixtureTree::new();
        t.mkdir("etc/wine");
        let s = sys(
            &t,
            vec![pacman("wine-staging", &["wine"])],
            owned(&["/usr/bin/true"]),
            &[],
        );
        assert!(s.find(0, false).is_empty());
        let fs = s.find(0, true);
        assert_eq!(fs.len(), 1, "{fs:#?}");
        assert_eq!(fs[0].confidence, Confidence::Low);
    }

    #[test]
    fn another_apps_low_claim_marks_shared() {
        let t = FixtureTree::new();
        t.mkdir("etc/brave");
        let s = sys(
            &t,
            vec![pacman("brave", &[]), pacman("Brave", &["x-brave"])],
            owned(&["/usr/bin/true"]),
            &[],
        );
        let fs = s.find(0, true);
        assert_eq!(fs.len(), 1, "{fs:#?}");
        assert!(fs[0].shared);
    }

    #[test]
    fn app_name_sets_lists_exact_names_and_keys() {
        let mut app = pacman("inky-app", &["inky"]);
        app.desktop_ids = vec!["org.inky.Inky.desktop".into()];
        app.rdns_id = Some("org.inky.Inky".into());
        let (names, keys) = crate::leftovers::system::app_name_sets(&app);
        for n in ["inky-app", "org.inky.Inky", "inky"] {
            assert!(names.iter().any(|x| x == n), "{n} in {names:?}");
        }
        assert_eq!(names.len(), keys.len());
        assert!(keys.iter().any(|k| k.norm == "inkyapp"));
    }
}
