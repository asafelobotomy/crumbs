use super::*;
use crate::config::Config;
use crate::desktop::discover_desktop_entries;
use crate::model::Installation;
use crate::sources::ListOptions;
use crate::testing::{FakeRunner, FixtureTree};

fn list(t: &FixtureTree) -> SourceListing {
    list_with(t, &Config::default())
}

fn list_with(t: &FixtureTree, cfg: &Config) -> SourceListing {
    let env = t.env();
    let desktop = discover_desktop_entries(&env);
    let runner = FakeRunner::new();
    let opts = ListOptions::default();
    let cx = SourceContext {
        env: &env,
        runner: &runner,
        desktop: &desktop,
        opts: &opts,
        config: cfg,
    };
    AppImageSource.list(&cx).unwrap()
}

#[test]
fn split_names() {
    let cases = [
        ("emobie_0.8.2_amd64.AppImage", "emobie", Some("0.8.2")),
        ("Grok_Bot_0.79.2.AppImage", "Grok_Bot", Some("0.79.2")),
        (
            "GutCheck-0.19.0-x86_64.AppImage",
            "GutCheck",
            Some("0.19.0"),
        ),
        ("Pass_Broker-x86_64.AppImage", "Pass_Broker", None),
        ("tool-v2.1.AppImage", "tool", Some("2.1")),
        ("Foo-1.0-x86_64.appimage", "Foo", Some("1.0")),
    ];
    for (file, name, ver) in cases {
        assert_eq!(
            split_appimage_name(file),
            (name.to_string(), ver.map(String::from)),
            "{file}"
        );
    }
}

#[test]
fn normalizes() {
    assert_eq!(normalize_name("Grok_Bot-1.x"), "grokbot1x");
}

#[test]
fn magic_detection() {
    let mut ok = vec![0x7f, b'E', b'L', b'F', 0, 0, 0, 0, b'A', b'I', 0x02];
    ok.resize(16, 0);
    assert!(is_appimage(&ok));
    let mut elf = ok.clone();
    elf[8] = 0;
    assert!(!is_appimage(&elf));
    assert!(!is_appimage(&[0x7f, b'E', b'L']));
}

#[test]
fn lists_appimages_in_applications_dir() {
    let t = FixtureTree::new();
    t.appimage("/home/user/Applications/GutCheck-0.19.0-x86_64.AppImage");
    let l = list(&t);
    assert_eq!(l.apps.len(), 1);
    let a = &l.apps[0];
    assert_eq!(
        a.id,
        "appimage:~/Applications/GutCheck-0.19.0-x86_64.AppImage"
    );
    assert_eq!(a.display_name, "GutCheck");
    assert_eq!(a.version.as_deref(), Some("0.19.0"));
    assert_eq!(a.installation, Installation::User);
    assert_eq!(a.install_size, Some(256));
    assert!(a.has_gui);
    let env = t.env();
    assert_eq!(
        a.location.as_deref(),
        Some(
            env.unsys(&env.home)
                .join("Applications/GutCheck-0.19.0-x86_64.AppImage")
                .as_path()
        )
    );
}

#[test]
fn hostile_dir_entries_skipped() {
    let t = FixtureTree::new();
    t.appimage("/home/user/Applications/real.AppImage");
    t.symlink("/home/user/Applications/broken.AppImage", "/nonexistent");
    t.symlink("/home/user/Applications/link.AppImage", "real.AppImage");
    t.mkdir("/home/user/Applications/x.AppImage");
    t.write("/home/user/Applications/empty.AppImage", b"");
    t.write(
        "/home/user/Applications/text.AppImage",
        b"hello world, not elf\n",
    );
    let l = list(&t);
    let ids: Vec<_> = l.apps.iter().map(|a| a.id.as_str()).collect();
    assert_eq!(ids, ["appimage:~/Applications/real.AppImage"]);
}

#[test]
fn extension_required_only_in_downloads_and_local_bin() {
    let t = FixtureTree::new();
    t.appimage("/home/user/Downloads/noext");
    t.appimage("/home/user/.local/bin/noext2");
    t.appimage("/home/user/.local/bin/yes.AppImage");
    t.appimage("/home/user/opt/noext3");
    let l = list(&t);
    let mut ids: Vec<_> = l.apps.iter().map(|a| a.id.as_str()).collect();
    ids.sort();
    assert_eq!(
        ids,
        [
            "appimage:~/.local/bin/yes.AppImage",
            "appimage:~/opt/noext3"
        ]
    );
}

#[test]
fn links_desktop_by_exec_path() {
    let t = FixtureTree::new();
    t.appimage("/home/user/Applications/Foo-1.0-x86_64.AppImage");
    t.desktop(
            "/home/user/.local/share/applications/whatever.desktop",
            "[Desktop Entry]\nType=Application\nName=Pretty Foo\nExec=~/Applications/Foo-1.0-x86_64.AppImage %U\nX-AppImage-Version=1.0\n",
        );
    t.desktop(
        "/home/user/.local/share/applications/other.desktop",
        "[Desktop Entry]\nType=Application\nName=Other\nExec=/usr/bin/other\n",
    );
    let l = list(&t);
    let a = &l.apps[0];
    assert_eq!(a.desktop_ids, ["whatever.desktop"]);
    assert_eq!(a.display_name, "Pretty Foo");
    assert_eq!(a.exec_names, ["Foo-1.0-x86_64.AppImage"]);
    assert!(l.claimed_desktop_ids.contains("whatever.desktop"));
    assert!(!l.claimed_desktop_ids.contains("other.desktop"));
}

#[test]
fn links_absolute_exec_path() {
    let t = FixtureTree::new();
    t.appimage("/home/user/opt/Bar.AppImage");
    t.desktop(
        "/usr/share/applications/bar.desktop",
        "[Desktop Entry]\nType=Application\nName=Bar\nExec=/home/user/opt/Bar.AppImage\n",
    );
    let l = list(&t);
    assert_eq!(l.apps[0].desktop_ids, ["bar.desktop"]);
}

#[test]
fn links_appofholding_style_entry() {
    let t = FixtureTree::new();
    t.appimage("/home/user/Applications/GutCheck-0.19.0-x86_64.AppImage");
    t.appimage("/home/user/Applications/Pass_Broker-x86_64.AppImage");
    t.desktop(
            "/home/user/.local/share/applications/gutcheck.desktop",
            "[Desktop Entry]\nType=Application\nName=GutCheck\nExec=/usr/bin/appofholding run gutcheck %U\nX-AppImage-Version=0.19.0\n",
        );
    t.desktop(
            "/home/user/.local/share/applications/pass-broker.desktop",
            "[Desktop Entry]\nType=Application\nName=Pass Broker\nExec=/usr/bin/appofholding run pass-broker\nX-AppImage-Version=2.3\n",
        );
    t.desktop(
        "/usr/share/applications/gutcheck-native.desktop",
        "[Desktop Entry]\nType=Application\nName=GutCheck\nExec=/usr/bin/gutcheck\n",
    );
    let l = list(&t);
    let gut = l
        .apps
        .iter()
        .find(|a| a.name.starts_with("GutCheck"))
        .unwrap();
    assert_eq!(gut.desktop_ids, ["gutcheck.desktop"]);
    assert_eq!(gut.exec_names, ["appofholding"]);
    assert!(l.claimed_desktop_ids.contains("gutcheck.desktop"));
    assert!(!l.claimed_desktop_ids.contains("gutcheck-native.desktop"));
    let pb = l
        .apps
        .iter()
        .find(|a| a.name.starts_with("Pass_Broker"))
        .unwrap();
    assert_eq!(pb.desktop_ids, ["pass-broker.desktop"]);
    assert_eq!(pb.display_name, "Pass Broker");
    assert_eq!(pb.version.as_deref(), Some("2.3"));
}

#[test]
fn appofholding_library_left_to_it() {
    let t = FixtureTree::new();
    t.appimage("/home/user/Applications/GutCheck-0.19.0-x86_64.AppImage");
    t.appimage("/home/user/Downloads/Foo-1.0-x86_64.AppImage");
    assert_eq!(list(&t).apps.len(), 2);
    // Once appOFholding is in use, ~/Applications is its library.
    t.write("home/user/.local/share/appofholding/db.sqlite", b"");
    let l = list(&t);
    let ids: Vec<&str> = l.apps.iter().map(|a| a.id.as_str()).collect();
    assert_eq!(ids, ["appimage:~/Downloads/Foo-1.0-x86_64.AppImage"]);
}

#[test]
fn appimagelauncher_destination_scanned() {
    let t = FixtureTree::new();
    t.write(
        "/home/user/.config/appimagelauncher.cfg",
        b"[AppImageLauncher]\nenable_daemon = false\ndestination = ~/Software\n",
    );
    t.appimage("/home/user/Software/Baz.AppImage");
    let l = list(&t);
    assert_eq!(l.apps.len(), 1);
    assert_eq!(l.apps[0].id, "appimage:~/Software/Baz.AppImage");
}

#[test]
fn launcher_destination_equal_to_default_not_duplicated() {
    let t = FixtureTree::new();
    t.write(
        "/home/user/.config/appimagelauncher.cfg",
        b"[AppImageLauncher]\ndestination = ~/Applications\n",
    );
    t.appimage("/home/user/Applications/Baz.AppImage");
    assert_eq!(list(&t).apps.len(), 1);
}

#[test]
fn appimage_ids_include_path() {
    let t = FixtureTree::new();
    t.appimage("/home/user/Applications/X-1.0-x86_64.AppImage");
    t.appimage("/home/user/Downloads/X-1.0-x86_64.AppImage");
    let l = list(&t);
    let ids: Vec<_> = l.apps.iter().map(|a| a.id.as_str()).collect();
    assert_eq!(
        ids,
        [
            "appimage:~/Applications/X-1.0-x86_64.AppImage",
            "appimage:~/Downloads/X-1.0-x86_64.AppImage"
        ]
    );
    assert_eq!(l.apps[0].name, "X-1.0-x86_64.AppImage");
}

#[test]
fn appimage_dirs_scanned() {
    let t = FixtureTree::new();
    t.appimage("/home/user/Tools/Extra-1.0-x86_64.AppImage");
    t.appimage("/home/user/Applications/Own.AppImage");
    t.appimage("/mnt/apps/Abs.AppImage");
    assert_eq!(list(&t).apps.len(), 1);
    let cfg = Config {
        appimage_dirs: vec![
            PathBuf::from("/home/user/Tools"),
            PathBuf::from("/home/user/Applications"),
            PathBuf::from("/mnt/apps"),
        ],
        ..Config::default()
    };
    let l = list_with(&t, &cfg);
    let ids: Vec<_> = l.apps.iter().map(|a| a.id.as_str()).collect();
    assert_eq!(
        ids,
        [
            "appimage:/mnt/apps/Abs.AppImage",
            "appimage:~/Applications/Own.AppImage",
            "appimage:~/Tools/Extra-1.0-x86_64.AppImage"
        ]
    );
}

#[test]
fn type1_magic_detected() {
    let mut b = vec![0x7f, b'E', b'L', b'F', 0, 0, 0, 0, b'A', b'I', 0x01];
    b.resize(16, 0);
    assert!(is_appimage(&b));
    b[10] = 0x03;
    assert!(!is_appimage(&b));
}
