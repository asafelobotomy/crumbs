use super::*;
use crate::config::Config;
use crate::desktop::discover_desktop_entries;
use crate::ownership::{load_dpkg, load_package_db};
use crate::sources::{AppSource, ListOptions, SourceContext};
use crate::testing::{FakeRunner, FixtureTree};
use std::path::Path;

const STATUS: &[u8] = include_bytes!("../../../tests/fixtures/dpkg/status");
const LISTS: [(&str, &[u8]); 4] = [
    (
        "coreutils.list",
        include_bytes!("../../../tests/fixtures/dpkg/info/coreutils.list"),
    ),
    (
        "libacl1:amd64.list",
        include_bytes!("../../../tests/fixtures/dpkg/info/libacl1:amd64.list"),
    ),
    (
        "firefox-esr.list",
        include_bytes!("../../../tests/fixtures/dpkg/info/firefox-esr.list"),
    ),
    (
        "oldpkg.list",
        include_bytes!("../../../tests/fixtures/dpkg/info/oldpkg.list"),
    ),
];
const FIREFOX_DESKTOP: &str = "[Desktop Entry]\nType=Application\nName=Firefox ESR\nExec=/usr/lib/firefox-esr/firefox-esr %u\n";

fn debian() -> FixtureTree {
    let t = FixtureTree::new();
    t.write("/var/lib/dpkg/status", STATUS);
    for (name, bytes) in LISTS {
        t.write(&format!("/var/lib/dpkg/info/{name}"), bytes);
    }
    t.write(
        "/usr/share/applications/firefox-esr.desktop",
        FIREFOX_DESKTOP.as_bytes(),
    );
    t
}

fn list(t: &FixtureTree, include_all: bool) -> SourceListing {
    let env = t.env();
    let desktop = discover_desktop_entries(&env);
    let runner = FakeRunner::new();
    let opts = ListOptions { include_all };
    let cx = SourceContext {
        env: &env,
        runner: &runner,
        desktop: &desktop,
        opts: &opts,
        config: &Config::default(),
    };
    DpkgSource.list(&cx).unwrap()
}

#[test]
fn dpkg_lists_gui_packages_from_status() {
    let t = debian();
    let l = list(&t, false);
    let ids: Vec<&str> = l.apps.iter().map(|a| a.id.as_str()).collect();
    assert_eq!(ids, ["dpkg:firefox-esr"]);
    let ff = &l.apps[0];
    assert_eq!(ff.version.as_deref(), Some("140.4.0esr-1"));
    assert_eq!(ff.display_name, "Firefox ESR");
    assert_eq!(ff.desktop_ids, ["firefox-esr.desktop"]);
    assert!(l.claimed_desktop_ids.contains("firefox-esr.desktop"));
    let all = list(&t, true);
    let mut names: Vec<&str> = all.apps.iter().map(|a| a.name.as_str()).collect();
    names.sort();
    assert_eq!(names, ["coreutils", "firefox-esr", "libacl1"]);
    assert!(l.errors.is_empty(), "{:?}", l.errors);
}

#[test]
fn dpkg_held_packages_are_listed() {
    let t = debian();
    let status = String::from_utf8_lossy(STATUS).replace(
        "Package: coreutils\nEssential: yes\nStatus: install ok installed",
        "Package: coreutils\nEssential: yes\nStatus: hold ok installed",
    );
    t.write("/var/lib/dpkg/status", status.as_bytes());
    let all = list(&t, true);
    assert!(all.apps.iter().any(|a| a.name == "coreutils"));
}

#[test]
fn dpkg_installed_package_without_list_is_incomplete() {
    let t = debian();
    std::fs::remove_file(t.env().sys("/var/lib/dpkg/info/coreutils.list")).unwrap();
    let l = list(&t, true);
    assert!(!l.errors.is_empty());
    assert!(load_dpkg(&t.env()).is_err());
}

#[test]
fn dpkg_diverted_paths_are_owned() {
    let t = debian();
    t.write(
        "/var/lib/dpkg/diversions",
        b"/etc/foo.conf\n/etc/foo.conf.distrib\nfoo-tweaks\n",
    );
    let idx = load_dpkg(&t.env()).unwrap().unwrap();
    assert!(idx.owns(Path::new("/etc/foo.conf.distrib")));
    assert!(
        list(&t, true)
            .owned_paths
            .contains(Path::new("/etc/foo.conf.distrib"))
    );
}

#[test]
fn dpkg_rc_packages_not_listed_but_owned() {
    let t = debian();
    let all = list(&t, true);
    assert!(!all.apps.iter().any(|a| a.name == "oldpkg"));
    assert!(
        all.owned_paths
            .contains(Path::new("/etc/oldpkg/oldpkg.conf"))
    );
}

#[test]
fn dpkg_multiarch_list_files_are_read() {
    let t = debian();
    let all = list(&t, true);
    assert!(
        all.owned_paths
            .contains(Path::new("/usr/lib/x86_64-linux-gnu/libacl.so.1"))
    );
    assert!(!all.owned_paths.contains(Path::new("/.")));
}

#[test]
fn dpkg_install_size_is_kib_times_1024() {
    let t = debian();
    let all = list(&t, true);
    let core = all.apps.iter().find(|a| a.name == "coreutils").unwrap();
    assert_eq!(core.install_size, Some(18457 * 1024));
}

#[test]
fn dpkg_unreadable_list_makes_index_incomplete() {
    let t = debian();
    let list_path = t.env().sys("/var/lib/dpkg/info/coreutils.list");
    std::fs::remove_file(&list_path).unwrap();
    std::fs::create_dir(&list_path).unwrap();
    let l = list(&t, true);
    assert!(!l.errors.is_empty());
    assert!(load_dpkg(&t.env()).is_err());
}

#[test]
fn load_package_db_on_debian_fixture_owns_usr_bin_ls() {
    let t = debian();
    let idx = load_package_db(&t.env(), &FakeRunner::new())
        .unwrap()
        .unwrap();
    assert!(idx.is_complete());
    assert!(idx.owns(Path::new("/usr/bin/cat")));
    assert!(idx.owns_at_or_below(Path::new("/usr/lib/firefox-esr")));
    assert!(!idx.owns_at_or_below(Path::new("/opt/inky")));
}
