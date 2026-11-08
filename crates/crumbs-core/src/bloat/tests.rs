#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use super::*;
use crate::model::{App, SourceKind};
use crate::safety::{is_allowed_target, verify};
use crate::testing::{FakeRunner, FixtureTree};

fn inv(apps: Vec<App>) -> Inventory {
    Inventory {
        apps,
        statuses: vec![],
        errors: vec![],
    }
}

fn run(t: &FixtureTree, cfg: &Config, inv: &Inventory, cat: Category) -> Vec<BloatItem> {
    let env = t.env();
    let runner = FakeRunner::new();
    let cx = ScanContext {
        env: &env,
        cfg,
        inv,
        runner: &runner,
        now: SystemTime::now(),
    };
    scanners()
        .into_iter()
        .find(|s| s.category() == cat)
        .unwrap()
        .scan(&cx)
        .unwrap()
}

fn paths(items: &[BloatItem]) -> Vec<PathBuf> {
    items.iter().map(|i| i.path.clone().unwrap()).collect()
}

fn check_common(t: &FixtureTree, cfg: &Config, items: &[BloatItem]) {
    let env = t.env();
    for i in items {
        let p = i.path.as_ref().unwrap();
        assert_eq!(i.scope, Scope::Home);
        assert_eq!(i.action, ItemAction::Trash);
        assert_eq!(i.size, None);
        assert!(is_allowed_target(&env, cfg, Scope::Home, p));
        verify(&env, cfg, Scope::Home, p, i.fingerprint.as_ref().unwrap()).unwrap();
    }
}

#[test]
fn cache_items_per_subdir_excluding_others() {
    let t = FixtureTree::new();
    for d in [
        "alpha",
        "thumbnails",
        "crumbs",
        "mozilla",
        "chromium",
        "google-chrome",
        "BraveSoftware",
        "vivaldi",
        "microsoft-edge",
        "pip",
        "go-build",
        "pnpm",
        "yarn",
        "ccache",
    ] {
        t.write(&format!("home/user/.cache/{d}/f"), b"x");
    }
    t.write("home/user/.cache/loose.txt", b"x");
    let cfg = Config::default();
    let items = run(&t, &cfg, &inv(vec![]), Category::Cache);
    assert_eq!(
        paths(&items),
        [
            PathBuf::from("/home/user/.cache/alpha"),
            PathBuf::from("/home/user/.cache/loose.txt")
        ]
    );
    assert_eq!(items[0].label, "alpha");
    assert!(items.iter().all(|i| i.rebuildable && i.preselected));
    assert!(items.iter().all(|i| i.category == Category::Cache));
    check_common(&t, &cfg, &items);
}

#[test]
fn cache_symlink_counts_link_only() {
    let t = FixtureTree::new();
    t.write("home/user/Keep/data", b"precious");
    t.symlink("home/user/.cache/linky", "/home/user/Keep");
    let cfg = Config::default();
    let items = run(&t, &cfg, &inv(vec![]), Category::Cache);
    assert_eq!(paths(&items), [PathBuf::from("/home/user/.cache/linky")]);
    assert!(items[0].fingerprint.unwrap().is_symlink);
    check_common(&t, &cfg, &items);
}

#[test]
fn cache_running_app_not_preselected() {
    let t = FixtureTree::new();
    t.write("home/user/.cache/hauler/f", b"x");
    t.write("home/user/.cache/other/f", b"x");
    t.symlink("proc/321/exe", "/usr/bin/hauler");
    let mut app = App::new(SourceKind::Pacman, "hauler");
    app.display_name = "Hauler".into();
    app.exec_names = vec!["hauler".into()];
    let items = run(
        &t,
        &Config::default(),
        &inv(vec![app.clone()]),
        Category::Cache,
    );
    let hauler = items.iter().find(|i| i.label == "hauler").unwrap();
    assert!(!hauler.preselected);
    assert_eq!(hauler.note.as_deref(), Some("Hauler is running"));
    assert_eq!(hauler.app_id.as_deref(), Some(app.id.as_str()));
    let other = items.iter().find(|i| i.label == "other").unwrap();
    assert!(other.preselected && other.note.is_none() && other.app_id.is_none());

    // Not running: owner is recorded, item stays preselected.
    let t2 = FixtureTree::new();
    t2.write("home/user/.cache/hauler/f", b"x");
    let items = run(
        &t2,
        &Config::default(),
        &inv(vec![app.clone()]),
        Category::Cache,
    );
    assert!(items[0].preselected && items[0].note.is_none());
    assert_eq!(items[0].app_id.as_deref(), Some(app.id.as_str()));

    // Two owners, one running: no app_id, but still not preselected.
    let mut twin = App::new(SourceKind::Flatpak, "hauler");
    twin.exec_names = vec!["hauler".into()];
    let both = inv(vec![app, twin]);
    let items = run(&t, &Config::default(), &both, Category::Cache);
    let hauler = items.iter().find(|i| i.label == "hauler").unwrap();
    assert!(hauler.app_id.is_none());
    assert!(!hauler.preselected);
    assert_eq!(hauler.note.as_deref(), Some("Hauler is running"));

    // Two owners, none running: ambiguous, preselected.
    let items = run(&t2, &Config::default(), &both, Category::Cache);
    assert!(items[0].app_id.is_none() && items[0].preselected && items[0].note.is_none());
}

#[test]
fn thumbnails_single_item() {
    let t = FixtureTree::new();
    let cfg = Config::default();
    assert!(run(&t, &cfg, &inv(vec![]), Category::Thumbnails).is_empty());
    t.write("home/user/.cache/thumbnails/normal/a.png", b"x");
    let items = run(&t, &cfg, &inv(vec![]), Category::Thumbnails);
    assert_eq!(
        paths(&items),
        [PathBuf::from("/home/user/.cache/thumbnails")]
    );
    assert_eq!(items[0].label, "thumbnails");
    assert!(items[0].rebuildable && items[0].preselected);
    check_common(&t, &cfg, &items);
}

#[test]
fn dev_cache_paths_listed_rebuildable() {
    let t = FixtureTree::new();
    assert!(run(&t, &Config::default(), &inv(vec![]), Category::DevCache).is_empty());
    for d in [
        ".cargo/registry/cache",
        ".cargo/registry/src",
        ".cargo/git/checkouts",
        ".npm/_cacache",
        ".local/share/pnpm/store",
        ".cache/pnpm",
        ".cache/yarn",
        ".cache/pip",
        ".cache/go-build",
        "go/pkg/mod/cache",
        ".gradle/caches",
        ".m2/repository",
        ".cache/ccache",
        ".ccache",
    ] {
        t.write(&format!("home/user/{d}/f"), b"x");
    }
    t.mkdir("home/user/.cargo/registry/index"); // not a listed path
    let cfg = Config::default();
    let items = run(&t, &cfg, &inv(vec![]), Category::DevCache);
    assert_eq!(items.len(), 14);
    assert!(items.iter().all(|i| i.rebuildable && i.preselected));
    let labels: Vec<_> = items.iter().map(|i| i.label.as_str()).collect();
    for l in [
        "cargo", "npm", "pnpm", "yarn", "pip", "go", "gradle", "maven", "ccache",
    ] {
        assert!(labels.contains(&l), "{l}");
    }
    assert_eq!(labels.iter().filter(|l| **l == "ccache").count(), 2);
    assert!(!paths(&items).contains(&PathBuf::from("/home/user/.cargo/registry/index")));
    check_common(&t, &cfg, &items);
}

fn age(p: &Path, days: u64) {
    let f = fs::File::options().write(true).open(p).unwrap();
    f.set_modified(SystemTime::now() - Duration::from_secs(days * 86_400 + 3600))
        .unwrap();
}

#[test]
fn logs_age_filter() {
    let t = FixtureTree::new();
    let old = t.write("home/user/.local/state/app/old.log", b"x");
    t.write("home/user/.local/state/app/fresh.log", b"x");
    let nested = t.write("home/user/.local/state/a/b/c/deep.log", b"x");
    let other = t.write("home/user/.local/state/app/notes.txt", b"x");
    let xs = t.write("home/user/.xsession-errors", b"x");
    let xs_old = t.write("home/user/.xsession-errors.old", b"x");
    t.write("home/user/.local/state/new/x.log", b"x");
    t.write("home/user/Keep/data.log", b"x");
    t.symlink("home/user/.local/state/link", "/home/user/Keep");
    t.symlink(
        "home/user/.local/state/app/sym.log",
        "/home/user/Keep/data.log",
    );
    for p in [&old, &nested, &other, &xs, &xs_old] {
        age(p, 30);
    }
    // Keep .xsession-errors.old fresh.
    fs::File::options()
        .write(true)
        .open(&xs_old)
        .unwrap()
        .set_modified(SystemTime::now())
        .unwrap();
    let cfg = Config::default();
    let items = run(&t, &cfg, &inv(vec![]), Category::Logs);
    let mut got = paths(&items);
    got.sort();
    assert_eq!(
        got,
        [
            PathBuf::from("/home/user/.local/state/a/b/c/deep.log"),
            PathBuf::from("/home/user/.local/state/app/old.log"),
            PathBuf::from("/home/user/.xsession-errors"),
        ]
    );
    assert!(items.iter().all(|i| !i.rebuildable));
    check_common(&t, &cfg, &items);
}

#[test]
fn protected_or_excluded_never_listed() {
    let t = FixtureTree::new();
    t.write("home/user/.cache/keepme/f", b"x");
    t.write("home/user/.cache/plain/f", b"x");
    let old = t.write("home/user/.local/state/crumbs/history.log", b"x");
    let excl = t.write("home/user/.local/state/skip/x.log", b"x");
    age(&old, 40);
    age(&excl, 40);
    t.write("home/user/.cargo/registry/cache/f", b"x");
    let cfg = Config {
        exclude: vec![
            "/home/user/.cache/keepme".into(),
            "/home/user/.local/state/skip/*".into(),
        ],
        protect: vec![PathBuf::from("/home/user/.cargo/registry")],
        ..Config::default()
    };
    let i = inv(vec![]);
    assert_eq!(
        paths(&run(&t, &cfg, &i, Category::Cache)),
        [PathBuf::from("/home/user/.cache/plain")]
    );
    assert!(run(&t, &cfg, &i, Category::Logs).is_empty());
    assert!(run(&t, &cfg, &i, Category::DevCache).is_empty());
}
