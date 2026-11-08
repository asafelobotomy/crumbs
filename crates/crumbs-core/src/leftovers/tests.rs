#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};

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
    t.mkdir("/home/user/.local/share/foo_1.4.2");
    let r = Run::new(&t, vec![pacman("foo", &[])]);
    let fs = r.find(0, false);
    assert_eq!(must(&fs, ".config/Foo-2").confidence, Confidence::Medium);
    let d = must(&fs, ".local/share/foo_1.4.2");
    assert_eq!(d.confidence, Confidence::Medium);
    assert_eq!(d.kind, FindingKind::Data);
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
