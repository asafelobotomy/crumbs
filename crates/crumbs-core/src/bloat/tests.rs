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
        owned: Default::default(),
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
        now: test_now(),
    };
    scanners()
        .into_iter()
        .find(|s| s.category() == cat)
        .unwrap()
        .scan(&cx)
        .unwrap()
}

/// The scans' clock in these tests: 100 days ahead, so an entry made now
/// already has a 100-day-old ctime (which cannot be set) and its mtime,
/// set relative to this clock, decides its age.
const CLOCK_SHIFT: Duration = Duration::from_secs(100 * 86_400);

fn test_now() -> SystemTime {
    SystemTime::now() + CLOCK_SHIFT
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
fn cache_git_checkout_not_preselected() {
    // Someone's work kept under ~/.cache: a git working tree (a `.git`
    // directory with an index) is never a cache, at any of the first
    // three levels. A bare `.git` without an index is a cache's own
    // clone and does not count.
    let t = FixtureTree::new();
    t.write("home/user/.cache/work/repo/.git/index", b"DIRC");
    t.write("home/user/.cache/work/repo/src/main.rs", b"x");
    t.write("home/user/.cache/top/.git/index", b"DIRC");
    t.write("home/user/.cache/deep/a/b/.git/index", b"DIRC");
    t.write("home/user/.cache/tool/db/x/.git/HEAD", b"ref");
    t.write("home/user/.cache/plain/f", b"x");
    let cfg = Config::default();
    let items = run(&t, &cfg, &inv(vec![]), Category::Cache);
    let item = |l: &str| items.iter().find(|i| i.label == l).unwrap();
    for (label, inner) in [("work", "repo"), ("top", "."), ("deep", "a/b")] {
        let i = item(label);
        assert!(!i.preselected, "{label}");
        assert!(!i.rebuildable, "{label}");
        assert_eq!(
            i.note.as_deref(),
            Some(format!("contains a git checkout ({inner})").as_str()),
            "{label}"
        );
    }
    for label in ["tool", "plain"] {
        let i = item(label);
        assert!(
            i.preselected && i.rebuildable && i.note.is_none(),
            "{label}"
        );
    }
    check_common(&t, &cfg, &items);
}

#[test]
fn cache_known_git_caches_stay_preselected() {
    // AUR helpers and pre-commit keep git checkouts as their cache, and a
    // folder tagged with CACHEDIR.TAG says it is a cache: all rebuildable.
    let t = FixtureTree::new();
    t.write("home/user/.cache/yay/foo/.git/index", b"DIRC");
    t.write("home/user/.cache/paru/clone/foo/.git/index", b"DIRC");
    t.write("home/user/.cache/pre-commit/repoab12/.git/index", b"DIRC");
    t.write("home/user/.cache/antidote/someone/.git/index", b"DIRC");
    t.write("home/user/.cache/tagged/repo/.git/index", b"DIRC");
    t.write(
        "home/user/.cache/tagged/CACHEDIR.TAG",
        b"Signature: 8a477f597d28d172789f06886806bc55\n# a cache\n",
    );
    t.write("home/user/.cache/badtag/repo/.git/index", b"DIRC");
    t.write("home/user/.cache/badtag/CACHEDIR.TAG", b"not a tag\n");
    let cfg = Config::default();
    let items = run(&t, &cfg, &inv(vec![]), Category::Cache);
    let item = |l: &str| items.iter().find(|i| i.label == l).unwrap();
    for label in ["yay", "paru", "pre-commit", "antidote", "tagged"] {
        let i = item(label);
        assert!(
            i.preselected && i.rebuildable && i.note.is_none(),
            "{label}: {i:?}"
        );
    }
    assert!(!item("badtag").preselected);
}

#[test]
fn cache_symlink_item_is_not_searched_for_git() {
    // A symlinked item: only the link would go, so what it points at is
    // not searched (it could be the whole home).
    let t = FixtureTree::new();
    t.write("home/user/work/repo/.git/index", b"DIRC");
    t.symlink("home/user/.cache/linky", "../work");
    let cfg = Config::default();
    let items = run(&t, &cfg, &inv(vec![]), Category::Cache);
    let i = items.iter().find(|i| i.label == "linky").unwrap();
    assert!(i.preselected && i.note.is_none(), "{i:?}");
}

#[test]
fn aur_build_lists_pamac_build_trees() {
    let t = FixtureTree::new();
    t.write(
        "etc/pamac.conf",
        b"# comment\nBuildDirectory = /var/tmp\nKeepNumPackages = 3\n",
    );
    t.write("var/tmp/pamac-build-user/brave/src/x", &[1u8; 5000]);
    t.write("var/tmp/pamac-build-user/chrome/pkg/y", &[1u8; 3000]);
    t.write("var/tmp/unrelated/z", b"z");
    let cfg = Config::default();
    let items = run(&t, &cfg, &inv(vec![]), Category::AurBuild);
    assert_eq!(paths(&items), [PathBuf::from("/var/tmp/pamac-build-user")]);
    let i = &items[0];
    assert_eq!(i.category, Category::AurBuild);
    assert_eq!(i.scope, Scope::UserTemp);
    assert_eq!(i.action, ItemAction::Delete);
    assert!(i.preselected && i.rebuildable);
    assert_eq!(i.label, "pamac build trees (2 packages)");
    assert_eq!(
        i.note.as_deref(),
        Some("deleted permanently; pamac rebuilds a package when it updates it")
    );
    let env = t.env();
    verify(
        &env,
        &cfg,
        Scope::UserTemp,
        &i.path.clone().unwrap(),
        i.fingerprint.as_ref().unwrap(),
    )
    .unwrap();
}

#[test]
fn aur_build_follows_pamac_build_directory() {
    // No pamac.conf: pamac's default, /var/tmp. A BuildDirectory of /tmp.
    let t = FixtureTree::new();
    t.write("var/tmp/pamac-build-user/a/x", b"x");
    let items = run(&t, &Config::default(), &inv(vec![]), Category::AurBuild);
    assert_eq!(paths(&items), [PathBuf::from("/var/tmp/pamac-build-user")]);
    let t = FixtureTree::new();
    t.write("etc/pamac.conf", b"BuildDirectory = /tmp\n");
    t.write("tmp/pamac-build-user/a/x", b"x");
    t.write("var/tmp/pamac-build-user/a/x", b"x");
    let items = run(&t, &Config::default(), &inv(vec![]), Category::AurBuild);
    assert_eq!(paths(&items), [PathBuf::from("/tmp/pamac-build-user")]);
    // Nothing built: nothing listed.
    let t = FixtureTree::new();
    t.mkdir("var/tmp");
    assert!(run(&t, &Config::default(), &inv(vec![]), Category::AurBuild).is_empty());
}

#[test]
fn aur_build_in_use_not_preselected() {
    let t = FixtureTree::new();
    t.write("var/tmp/pamac-build-user/brave/src/x", b"x");
    t.symlink("proc/77/cwd", "/var/tmp/pamac-build-user/brave/src");
    t.write("proc/77/comm", b"makepkg\n");
    let items = run(&t, &Config::default(), &inv(vec![]), Category::AurBuild);
    assert!(!items[0].preselected);
    assert_eq!(items[0].note.as_deref(), Some("in use by makepkg (pid 77)"));
}

#[test]
fn app_cache_lists_electron_app_caches() {
    let t = FixtureTree::new();
    t.write("home/user/.config/Slacky/Local State", b"{}");
    t.write("home/user/.config/Slacky/Cache/Cache_Data/a", &[1u8; 4000]);
    t.write("home/user/.config/Slacky/Code Cache/js/b", &[1u8; 4000]);
    t.write("home/user/.config/Slacky/GPUCache/c", b"c");
    t.write("home/user/.config/Slacky/DawnWebGPUCache/d", b"d");
    // Site data and settings are not caches.
    t.write("home/user/.config/Slacky/IndexedDB/e", b"e");
    t.write("home/user/.config/Slacky/Local Storage/f", b"f");
    // Not a Chromium app: no `Local State`.
    t.write("home/user/.config/plain/Cache/g", b"g");
    let cfg = Config::default();
    let items = run(&t, &cfg, &inv(vec![]), Category::AppCache);
    let mut got = paths(&items);
    got.sort();
    assert_eq!(
        got,
        [
            PathBuf::from("/home/user/.config/Slacky/Cache"),
            PathBuf::from("/home/user/.config/Slacky/Code Cache"),
            PathBuf::from("/home/user/.config/Slacky/DawnWebGPUCache"),
            PathBuf::from("/home/user/.config/Slacky/GPUCache"),
        ]
    );
    let i = items.iter().find(|i| i.label == "Slacky: Cache").unwrap();
    assert!(i.preselected && i.rebuildable && i.note.is_none());
    assert_eq!(i.category, Category::AppCache);
    for i in &items {
        assert_eq!(i.scope, Scope::Home);
        assert_eq!(i.action, ItemAction::Trash);
        let env = t.env();
        verify(
            &env,
            &cfg,
            Scope::Home,
            i.path.as_ref().unwrap(),
            i.fingerprint.as_ref().unwrap(),
        )
        .unwrap();
    }
}

#[test]
fn app_cache_running_app_not_preselected() {
    // Running: a process names the dir as its --user-data-dir, or holds a
    // file open in it.
    let t = FixtureTree::new();
    for app in ["ByFlag", "ByFd", "Idle"] {
        t.write(&format!("home/user/.config/{app}/Local State"), b"{}");
        t.write(&format!("home/user/.config/{app}/Cache/x"), b"x");
    }
    t.write(
        "proc/50/cmdline",
        b"/usr/lib/byflag/byflag\0--type=renderer\0--user-data-dir=/home/user/.config/ByFlag\0",
    );
    t.symlink("proc/60/fd/7", "/home/user/.config/ByFd/Cookies");
    let items = run(&t, &Config::default(), &inv(vec![]), Category::AppCache);
    let item = |l: &str| items.iter().find(|i| i.label == l).unwrap();
    for app in ["ByFlag", "ByFd"] {
        let i = item(&format!("{app}: Cache"));
        assert!(!i.preselected, "{app}");
        assert_eq!(
            i.note.as_deref(),
            Some(format!("{app} is running").as_str())
        );
    }
    assert!(item("Idle: Cache").preselected);
}

#[test]
fn aur_build_with_a_file_open_inside_not_preselected() {
    // pamac writing into the tree from elsewhere: an open file counts.
    let t = FixtureTree::new();
    t.write("var/tmp/pamac-build-user/brave/src/x", b"x");
    t.symlink("proc/66/fd/3", "/var/tmp/pamac-build-user/brave/src/x");
    t.write("proc/66/comm", b"pamac\n");
    let items = run(&t, &Config::default(), &inv(vec![]), Category::AurBuild);
    assert!(!items[0].preselected);
    assert_eq!(items[0].note.as_deref(), Some("in use by pamac (pid 66)"));
}

#[test]
fn cache_dir_in_use_not_preselected() {
    // A running process works inside it (its current directory), as a
    // build does: never preselected, and the note names the process.
    let t = FixtureTree::new();
    t.write("home/user/.cache/build/target/x", b"x");
    t.write("home/user/.cache/idle/x", b"x");
    t.symlink("proc/4242/cwd", "/home/user/.cache/build/target");
    t.write("proc/4242/comm", b"cargo\n");
    t.symlink("proc/77/cwd", "/home/user");
    t.write("proc/77/comm", b"zsh\n");
    let cfg = Config::default();
    let items = run(&t, &cfg, &inv(vec![]), Category::Cache);
    let build = items.iter().find(|i| i.label == "build").unwrap();
    assert!(!build.preselected);
    assert!(build.rebuildable);
    assert_eq!(build.note.as_deref(), Some("in use by cargo (pid 4242)"));
    let idle = items.iter().find(|i| i.label == "idle").unwrap();
    assert!(idle.preselected && idle.note.is_none());
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
    f.set_modified(test_now() - Duration::from_secs(days * 86_400 + 3600))
        .unwrap();
}

#[test]
fn logs_age_filter() {
    let t = FixtureTree::new();
    let old = t.write("home/user/.local/state/app/old.log", b"x");
    let fresh = t.write("home/user/.local/state/app/fresh.log", b"x");
    let nested = t.write("home/user/.local/state/a/b/c/deep.log", b"x");
    let other = t.write("home/user/.local/state/app/notes.txt", b"x");
    let xs = t.write("home/user/.xsession-errors", b"x");
    let xs_old = t.write("home/user/.xsession-errors.old", b"x");
    let fresh_new = t.write("home/user/.local/state/new/x.log", b"x");
    for p in [&fresh, &fresh_new] {
        age(p, 0);
    }
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
        .set_modified(test_now())
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

// ---- browser-cache, temp, trash ------------------------------------------

use crate::command::CmdOutput;

fn scan_one(
    t: &FixtureTree,
    cfg: &Config,
    runner: &FakeRunner,
    cat: Category,
) -> Result<Vec<BloatItem>, CoreError> {
    let env = t.env();
    let i = inv(vec![]);
    let cx = ScanContext {
        env: &env,
        cfg,
        inv: &i,
        runner,
        now: test_now(),
    };
    scanners()
        .into_iter()
        .find(|s| s.category() == cat)
        .unwrap()
        .scan(&cx)
}

/// [`scan_one`] at the real time, unshifted.
fn scan_real_now(
    t: &FixtureTree,
    cfg: &Config,
    runner: &FakeRunner,
    cat: Category,
) -> Result<Vec<BloatItem>, CoreError> {
    let env = t.env();
    let i = inv(vec![]);
    let cx = ScanContext {
        env: &env,
        cfg,
        inv: &i,
        runner,
        now: SystemTime::now(),
    };
    scanners()
        .into_iter()
        .find(|s| s.category() == cat)
        .unwrap()
        .scan(&cx)
}

fn labels(items: &[BloatItem]) -> Vec<&str> {
    items.iter().map(|i| i.label.as_str()).collect()
}

/// Sets atime and mtime of `p` (never following a symlink) to `days` (and
/// an hour) before [`test_now`].
fn age_nofollow(p: &Path, days: u64) {
    use rustix::fs::{AtFlags, CWD, Timespec, Timestamps, utimensat};
    let secs = test_now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_secs()
        - days * 86_400
        - 3600;
    let ts = Timespec {
        tv_sec: secs as i64,
        tv_nsec: 0,
    };
    utimensat(
        CWD,
        p,
        &Timestamps {
            last_access: ts,
            last_modification: ts,
        },
        AtFlags::SYMLINK_NOFOLLOW,
    )
    .unwrap();
}

fn check_browser(t: &FixtureTree, items: &[BloatItem]) {
    let cfg = Config::default();
    check_common(t, &cfg, items);
    for i in items {
        assert_eq!(i.category, Category::BrowserCache);
        assert!(i.rebuildable);
        assert!(i.extra_paths.is_empty());
    }
}

#[test]
fn browser_profiles_found() {
    let t = FixtureTree::new();
    let ff = "home/user/.cache/mozilla/firefox/ab12.default-release";
    t.write(&format!("{ff}/cache2/entries/e1"), b"x");
    t.write(&format!("{ff}/startupCache/s"), b"x");
    let brave = "home/user/.cache/BraveSoftware/Brave-Origin/Default";
    t.write(&format!("{brave}/Cache/Cache_Data/d"), b"x");
    t.write(&format!("{brave}/Code Cache/js/j"), b"x");
    t.write(&format!("{brave}/GPUCache/g"), b"x");
    t.write(&format!("{brave}/Other/o"), b"x");
    t.write("home/user/.cache/google-chrome/Profile 1/Cache/c", b"x");
    // A symlinked profile is never expanded.
    t.write("home/user/Keep/Cache/precious", b"x");
    t.symlink("home/user/.cache/google-chrome/Linked", "/home/user/Keep");
    // A symlinked cache dir, or a plain file where a dir belongs, is skipped.
    t.write("home/user/Keep2/precious", b"x");
    t.symlink(
        "home/user/.cache/chromium/Default/Cache",
        "/home/user/Keep2",
    );
    t.write("home/user/.cache/vivaldi/Default/GPUCache", b"file");
    let runner = FakeRunner::new();
    let items = scan_one(&t, &Config::default(), &runner, Category::BrowserCache).unwrap();
    let mut got: Vec<(PathBuf, String)> = items
        .iter()
        .map(|i| (i.path.clone().unwrap(), i.label.clone()))
        .collect();
    got.sort();
    let c = "/home/user/.cache";
    assert_eq!(
        got,
        [
            (
                PathBuf::from(format!("{c}/BraveSoftware/Brave-Origin/Default/Cache")),
                "Brave (Brave-Origin/Default) Cache".to_string()
            ),
            (
                PathBuf::from(format!("{c}/BraveSoftware/Brave-Origin/Default/Code Cache")),
                "Brave (Brave-Origin/Default) Code Cache".to_string()
            ),
            (
                PathBuf::from(format!("{c}/BraveSoftware/Brave-Origin/Default/GPUCache")),
                "Brave (Brave-Origin/Default) GPUCache".to_string()
            ),
            (
                PathBuf::from(format!("{c}/google-chrome/Profile 1/Cache")),
                "Chrome (Profile 1) Cache".to_string()
            ),
            (
                PathBuf::from(format!("{c}/mozilla/firefox/ab12.default-release/cache2")),
                "Firefox (ab12.default-release) cache2".to_string()
            ),
        ]
    );
    assert!(items.iter().all(|i| i.preselected && i.note.is_none()));
    check_browser(&t, &items);
    // No Flatpak cache dirs: flatpak is never asked.
    assert!(runner.calls().is_empty());
}

#[test]
fn browser_running_not_preselected() {
    let t = FixtureTree::new();
    t.write(
        "home/user/.cache/BraveSoftware/Brave-Browser/Default/Cache/x",
        b"x",
    );
    t.write("home/user/.cache/mozilla/firefox/p.default/cache2/x", b"x");
    t.symlink("proc/77/exe", "/opt/brave.com/brave/brave");
    t.symlink("proc/78/exe", "/usr/bin/bash");
    let items = scan_one(
        &t,
        &Config::default(),
        &FakeRunner::new(),
        Category::BrowserCache,
    )
    .unwrap();
    assert_eq!(items.len(), 2);
    let brave = items.iter().find(|i| i.label.starts_with("Brave")).unwrap();
    assert!(!brave.preselected);
    assert_eq!(brave.note.as_deref(), Some("Brave is running"));
    let ff = items
        .iter()
        .find(|i| i.label.starts_with("Firefox"))
        .unwrap();
    assert!(ff.preselected && ff.note.is_none());
    check_browser(&t, &items);

    // Brave-Origin runs as `brave-origin`.
    let t2 = FixtureTree::new();
    t2.write(
        "home/user/.cache/BraveSoftware/Brave-Origin/Default/Cache/x",
        b"x",
    );
    t2.symlink("proc/90/exe", "/opt/brave.com/brave-origin/brave-origin");
    let items = scan_one(
        &t2,
        &Config::default(),
        &FakeRunner::new(),
        Category::BrowserCache,
    )
    .unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].note.as_deref(), Some("Brave is running"));

    // A browser whose binary was replaced by an update still counts.
    t.symlink("proc/79/exe", "/usr/lib/firefox/firefox (deleted)");
    let items = scan_one(
        &t,
        &Config::default(),
        &FakeRunner::new(),
        Category::BrowserCache,
    )
    .unwrap();
    assert!(items.iter().all(|i| !i.preselected));
    let ff = items
        .iter()
        .find(|i| i.label.starts_with("Firefox"))
        .unwrap();
    assert_eq!(ff.note.as_deref(), Some("Firefox is running"));
}

#[test]
fn flatpak_browser_cache_found() {
    let t = FixtureTree::new();
    t.write_exec("usr/bin/flatpak", b"#!/bin/sh\n");
    let ff = "home/user/.var/app/org.mozilla.firefox/cache/mozilla/firefox/xy.default";
    t.write(&format!("{ff}/cache2/x"), b"x");
    let br = "home/user/.var/app/com.brave.Browser/cache/BraveSoftware/Brave-Browser/Default";
    t.write(&format!("{br}/GPUCache/x"), b"x");
    // Another app's pattern under a browser's Flatpak id is not matched.
    t.write(
        "home/user/.var/app/org.mozilla.firefox/cache/chromium/Default/Cache/x",
        b"x",
    );
    let ps = CmdOutput {
        status: 0,
        stdout: "com.brave.Browser\norg.other.App\n".into(),
        stderr: String::new(),
        timed_out: false,
    };
    let runner = FakeRunner::new().on("flatpak", &["ps", "--columns=application"], ps);
    let items = scan_one(&t, &Config::default(), &runner, Category::BrowserCache).unwrap();
    let mut got = paths(&items);
    got.sort();
    assert_eq!(
        got,
        [
            PathBuf::from(format!("/{br}/GPUCache")),
            PathBuf::from(format!("/{ff}/cache2")),
        ]
    );
    let brave = items.iter().find(|i| i.label.starts_with("Brave")).unwrap();
    assert_eq!(brave.label, "Brave (Brave-Browser/Default) GPUCache");
    assert!(!brave.preselected);
    assert_eq!(brave.note.as_deref(), Some("Brave is running"));
    let fox = items
        .iter()
        .find(|i| i.label.starts_with("Firefox"))
        .unwrap();
    assert_eq!(fox.label, "Firefox (xy.default) cache2");
    assert!(fox.preselected && fox.note.is_none());
    check_browser(&t, &items);
    // Asked once, however many Flatpak items there are.
    assert_eq!(runner.calls(), ["flatpak ps --columns=application"]);

    // `flatpak ps` failing, or no trusted flatpak: fail closed.
    let failing = FakeRunner::new().on(
        "flatpak",
        &["ps", "--columns=application"],
        CmdOutput {
            status: 1,
            stdout: String::new(),
            stderr: "boom".into(),
            timed_out: false,
        },
    );
    for runner in [FakeRunner::new(), failing] {
        let items = scan_one(&t, &Config::default(), &runner, Category::BrowserCache).unwrap();
        assert_eq!(items.len(), 2);
        for i in &items {
            assert!(!i.preselected);
            let browser = i.label.split(' ').next().unwrap();
            assert!(i.label.ends_with(" cache2") || i.label.ends_with(" GPUCache"));
            assert_eq!(
                i.note.as_deref(),
                Some(format!("could not check whether {browser} is running").as_str())
            );
        }
    }
    fs::remove_file(t.dir.path().join("usr/bin/flatpak")).unwrap();
    let runner = FakeRunner::new();
    let items = scan_one(&t, &Config::default(), &runner, Category::BrowserCache).unwrap();
    assert!(items.iter().all(|i| !i.preselected));
    assert!(runner.calls().is_empty());
}

fn check_temp(t: &FixtureTree, items: &[BloatItem]) {
    let env = t.env();
    let cfg = Config::default();
    for i in items {
        let p = i.path.as_ref().unwrap();
        assert_eq!(i.category, Category::Temp);
        assert_eq!(i.scope, Scope::UserTemp);
        assert_eq!(i.action, ItemAction::Delete);
        assert_eq!(
            i.note.as_deref(),
            Some("deleted permanently (temporary file)")
        );
        assert!(!i.rebuildable && i.preselected);
        assert!(is_allowed_target(&env, &cfg, Scope::UserTemp, p));
        verify(
            &env,
            &cfg,
            Scope::UserTemp,
            p,
            i.fingerprint.as_ref().unwrap(),
        )
        .unwrap();
    }
}

#[test]
fn temp_rules() {
    if crate::testing::running_as_root() {
        eprintln!("skipped: temp cleanup is not offered as root");
        return;
    }
    use rustix::fs::{CWD, FileType, Mode, mknodat};
    let t = FixtureTree::new();
    let tmp = t.mkdir("tmp");
    let sock = tmp.join("sock");
    let _listener = std::os::unix::net::UnixListener::bind(&sock).unwrap();
    let fifo = tmp.join("fifo");
    mknodat(CWD, &fifo, FileType::Fifo, Mode::from_raw_mode(0o600), 0).unwrap();
    let mut old: Vec<PathBuf> = vec![sock, fifo];
    for name in [
        ".X11-unix",
        ".ICE-unix",
        ".font-unix",
        "tmux-1000",
        "ssh-XXXXabc",
        "systemd-private-abc-colord.service-x",
        "pulse-abc",
    ] {
        old.push(t.write(&format!("tmp/{name}/f"), b"x"));
        old.push(tmp.join(name));
    }
    for name in ["krb5cc_1000", ".s.PGSQL.5432", ".X0-lock"] {
        old.push(t.write(&format!("tmp/{name}"), b"x"));
    }
    age_nofollow(&t.write("tmp/fresh", b"x"), 0);
    age_nofollow(&t.write("tmp/olddir-fresh/sub/new", b"x"), 0);
    old.push(tmp.join("olddir-fresh/sub"));
    old.push(tmp.join("olddir-fresh"));
    old.push(t.write("tmp/oldfile", b"x"));
    old.push(t.write("tmp/olddir-old/sub/a", b"x"));
    old.push(t.write("tmp/olddir-old/b", b"x"));
    old.push(tmp.join("olddir-old/sub"));
    old.push(tmp.join("olddir-old"));
    // A dir holding a socket may be live IPC: skipped.
    let inner = tmp.join("olddir-sock");
    fs::create_dir(&inner).unwrap();
    let _l2 = std::os::unix::net::UnixListener::bind(inner.join("s")).unwrap();
    old.push(inner.join("s"));
    old.push(inner);
    // An old symlink is never a candidate.
    t.symlink("tmp/oldlink", "/tmp/oldfile");
    old.push(tmp.join("oldlink"));
    for p in &old {
        age_nofollow(p, 30);
    }
    let items = scan_one(&t, &Config::default(), &FakeRunner::new(), Category::Temp).unwrap();
    assert_eq!(
        paths(&items),
        [
            PathBuf::from("/tmp/olddir-old"),
            PathBuf::from("/tmp/oldfile")
        ]
    );
    assert_eq!(labels(&items), ["/tmp/olddir-old", "/tmp/oldfile"]);
    check_temp(&t, &items);

    // The age threshold comes from the config.
    let cfg = Config {
        temp_age_days: 60,
        ..Config::default()
    };
    assert!(
        scan_one(&t, &cfg, &FakeRunner::new(), Category::Temp)
            .unwrap()
            .is_empty()
    );
    // Excluded paths are never listed.
    let cfg = Config {
        exclude: vec!["/tmp/oldfile".into()],
        ..Config::default()
    };
    assert_eq!(
        paths(&scan_one(&t, &cfg, &FakeRunner::new(), Category::Temp).unwrap()),
        [PathBuf::from("/tmp/olddir-old")]
    );
}

#[test]
fn temp_dir_over_cap_skipped() {
    if crate::testing::running_as_root() {
        eprintln!("skipped: temp cleanup is not offered as root");
        return;
    }
    let t = FixtureTree::new();
    let big = t.mkdir("tmp/big");
    for i in 0..10_001 {
        let p = big.join(format!("f{i}"));
        fs::write(&p, b"").unwrap();
        age_nofollow(&p, 30);
    }
    age_nofollow(&big, 30);
    let at_cap = t.mkdir("tmp/atcap");
    for i in 0..10_000 {
        let p = at_cap.join(format!("f{i}"));
        fs::write(&p, b"").unwrap();
        age_nofollow(&p, 30);
    }
    age_nofollow(&at_cap, 30);
    let items = scan_one(&t, &Config::default(), &FakeRunner::new(), Category::Temp).unwrap();
    assert_eq!(paths(&items), [PathBuf::from("/tmp/atcap")]);
}

#[test]
fn temp_owner_rule() {
    if crate::testing::running_as_root() {
        eprintln!("skipped: temp cleanup is not offered as root");
        return;
    }
    use std::os::unix::fs::MetadataExt;
    let t = FixtureTree::new();
    let f = t.write("tmp/f", b"x");
    let d = t.mkdir("tmp/d");
    t.symlink("tmp/l", "/tmp/f");
    let fm = fs::symlink_metadata(&f).unwrap();
    let dm = fs::symlink_metadata(&d).unwrap();
    let lm = fs::symlink_metadata(t.dir.path().join("tmp/l")).unwrap();
    let me = fm.uid();
    let now = SystemTime::now();
    let week = Duration::from_secs(7 * 86_400);
    let old = now - Duration::from_secs(30 * 86_400);
    let ok = |name: &str, owner: u32, ft: fs::FileType, mtime: SystemTime| {
        let st = super::temp::EntryStat {
            owner_uid: owner,
            file_type: ft,
            changed: mtime,
        };
        super::temp::temp_candidate(std::ffi::OsStr::new(name), &st, me, now, week)
    };
    assert!(ok("a", me, fm.file_type(), old));
    assert!(ok("a", me, dm.file_type(), old));
    // Someone else's file or dir.
    assert!(!ok("a", me.wrapping_add(1), fm.file_type(), old));
    assert!(!ok("a", 0, dm.file_type(), old));
    assert!(!ok("a", me, lm.file_type(), old));
    assert!(!ok("a", me, fm.file_type(), now));
    // mtime in the future is not old.
    assert!(!ok("a", me, fm.file_type(), now + week));
    assert!(!ok("tmux-1000", me, dm.file_type(), old));
    assert!(!ok("xauth_XYZ", me, fm.file_type(), old));
}

fn check_trash(t: &FixtureTree, items: &[BloatItem]) {
    let env = t.env();
    let cfg = Config::default();
    for i in items {
        assert_eq!(i.category, Category::Trash);
        assert_eq!(i.scope, Scope::HomeTrash);
        assert_eq!(i.action, ItemAction::Delete);
        assert!(
            !i.rebuildable && !i.preselected,
            "emptying the trash is opt-in"
        );
        let p = i.path.as_ref().unwrap();
        verify(
            &env,
            &cfg,
            Scope::HomeTrash,
            p,
            i.fingerprint.as_ref().unwrap(),
        )
        .unwrap();
        for (x, fp) in &i.extra_paths {
            verify(&env, &cfg, Scope::HomeTrash, x, fp).unwrap();
        }
    }
}

#[test]
fn trash_pairs_and_orphans() {
    let t = FixtureTree::new();
    let r = FakeRunner::new();
    let cfg = Config::default();
    // No trash yet: nothing to do, no error.
    assert!(scan_one(&t, &cfg, &r, Category::Trash).unwrap().is_empty());
    let tr = "home/user/.local/share/Trash";
    let info = |name: &str, path: &str| {
        let body = format!("[Trash Info]\nPath={path}\nDeletionDate=2026-01-01T00:00:00\n");
        t.write(&format!("{tr}/info/{name}.trashinfo"), body.as_bytes());
    };
    t.write(&format!("{tr}/files/a.txt"), b"aaaa");
    info("a.txt", "/home/user/My%20Docs/a.txt");
    t.write(&format!("{tr}/files/d/inner"), b"x");
    info("d", "/srv/caf%C3%A9");
    t.write(&format!("{tr}/files/x"), b"orphan file");
    info("y", "/home/user/y");
    t.write(&format!("{tr}/info/z.trashinfo"), b"[Trash Info]\n");
    t.write(&format!("{tr}/info/junk"), b"not a trashinfo");
    let items = scan_one(&t, &cfg, &r, Category::Trash).unwrap();
    let h = "/home/user/.local/share/Trash";
    let got: Vec<(PathBuf, Vec<PathBuf>, &str)> = items
        .iter()
        .map(|i| {
            (
                i.path.clone().unwrap(),
                i.extra_paths.iter().map(|(p, _)| p.clone()).collect(),
                i.label.as_str(),
            )
        })
        .collect();
    let pb = |s: &str| PathBuf::from(format!("{h}/{s}"));
    assert_eq!(
        got,
        [
            (
                pb("files/a.txt"),
                vec![pb("info/a.txt.trashinfo")],
                "~/My Docs/a.txt"
            ),
            (pb("files/d"), vec![pb("info/d.trashinfo")], "/srv/café"),
            (pb("files/x"), vec![], "x (no trash info)"),
            (pb("info/y.trashinfo"), vec![], "~/y (trash info only)"),
            (pb("info/z.trashinfo"), vec![], "z (trash info only)"),
        ]
    );
    check_trash(&t, &items);
    // Protected entries are never listed.
    let cfg = Config {
        protect: vec![pb("files/a.txt")],
        ..Config::default()
    };
    let items = scan_one(&t, &cfg, &r, Category::Trash).unwrap();
    assert!(!paths(&items).contains(&pb("files/a.txt")));
    assert_eq!(items.len(), 4);
}

#[test]
fn trash_symlinked_files_dir_is_error() {
    use std::os::unix::fs::PermissionsExt;
    let t = FixtureTree::new();
    let r = FakeRunner::new();
    let cfg = Config::default();
    let tr = t.mkdir("home/user/.local/share/Trash");
    t.mkdir("home/user/.local/share/Trash/info");
    t.write("elsewhere/precious", b"x");
    t.symlink("home/user/.local/share/Trash/files", "/elsewhere");
    assert!(scan_one(&t, &cfg, &r, Category::Trash).is_err());
    // A trash writable by others is refused too.
    fs::remove_file(tr.join("files")).unwrap();
    t.write("home/user/.local/share/Trash/files/x", b"x");
    assert_eq!(scan_one(&t, &cfg, &r, Category::Trash).unwrap().len(), 1);
    fs::set_permissions(&tr, fs::Permissions::from_mode(0o777)).unwrap();
    assert!(scan_one(&t, &cfg, &r, Category::Trash).is_err());
    fs::set_permissions(&tr, fs::Permissions::from_mode(0o700)).unwrap();
    // A missing `info` dir in an existing trash fails closed.
    fs::remove_dir(tr.join("info")).unwrap();
    assert!(scan_one(&t, &cfg, &r, Category::Trash).is_err());
}

#[test]
fn registry_follows_category_order() {
    let cats: Vec<Category> = scanners().iter().map(|s| s.category()).collect();
    assert!(cats.windows(2).all(|w| w[0] < w[1]), "{cats:?}");
    for c in [Category::Trash, Category::Temp, Category::BrowserCache] {
        assert!(cats.contains(&c), "{c:?}");
    }
}

#[test]
fn temp_extra_prefixes_kept() {
    if crate::testing::running_as_root() {
        eprintln!("skipped: temp cleanup is not offered as root");
        return;
    }
    let t = FixtureTree::new();
    let tmp = t.mkdir("tmp");
    let mut old = vec![];
    for name in [
        "xauth_abc",
        ".xauth123",
        "serverauth.x",
        "sddm-auth",
        "gdm-x",
        "lightdm-x",
        "kdeinit5__0",
        "plasma-csd-x",
        "dbus-abc",
        ".wine-1000",
        "nvidia-x",
        "mozilla_user0",
        "vscode-ipc",
        "pyright-x",
        "jetbrains-x",
    ] {
        old.push(t.write(&format!("tmp/{name}"), b"x"));
    }
    old.push(t.write("tmp/plain/sub/f", b"x"));
    old.push(tmp.join("plain/sub"));
    old.push(tmp.join("plain"));
    for p in &old {
        age_nofollow(p, 30);
    }
    let cfg = Config::default();
    let items = scan_one(&t, &cfg, &FakeRunner::new(), Category::Temp).unwrap();
    assert_eq!(paths(&items), [PathBuf::from("/tmp/plain")]);
    super::temp::still_stale(&t.env(), &cfg, Path::new("/tmp/plain"), test_now()).unwrap();
}

#[test]
fn temp_open_files_kept() {
    if crate::testing::running_as_root() {
        eprintln!("skipped: temp cleanup is not offered as root");
        return;
    }
    let t = FixtureTree::new();
    let tmp = t.mkdir("tmp");
    let lock = t.write("tmp/app.lock", b"x");
    let mapped = t.write("tmp/mapdir/lib.so", b"x");
    let cwd = t.mkdir("tmp/shell-here");
    let free = t.write("tmp/free", b"x");
    for p in [&lock, &mapped, &tmp.join("mapdir"), &cwd, &free] {
        age_nofollow(p, 30);
    }
    t.symlink("proc/50/fd/3", lock.to_str().unwrap());
    t.symlink("proc/50/fd/4", "/nonexistent/ignored");
    t.symlink("proc/52/cwd", cwd.to_str().unwrap());
    let m = fs::metadata(&mapped).unwrap();
    use std::os::unix::fs::MetadataExt;
    let line = format!(
        "7f00-7f10 r-xp 00000000 {:02x}:{:02x} {} /tmp/mapdir/lib.so\n00400000-00401000 r--p 00000000 00:00 0 [heap]\n",
        rustix::fs::major(m.dev()),
        rustix::fs::minor(m.dev()),
        m.ino()
    );
    t.write("proc/51/maps", line.as_bytes());
    let items = scan_one(&t, &Config::default(), &FakeRunner::new(), Category::Temp).unwrap();
    assert_eq!(paths(&items), [PathBuf::from("/tmp/free")]);
    let env = t.env();
    let err = super::temp::still_stale(
        &env,
        &Config::default(),
        Path::new("/tmp/app.lock"),
        test_now(),
    )
    .unwrap_err();
    assert_eq!(err, "is open in a running process");
}

#[test]
fn temp_refused_as_root() {
    let t = FixtureTree::new();
    let f = t.write("tmp/old", b"x");
    age_nofollow(&f, 30);
    let mut env = t.env();
    env.uid = 0;
    let cfg = Config::default();
    let i = inv(vec![]);
    let runner = FakeRunner::new();
    let cx = ScanContext {
        env: &env,
        cfg: &cfg,
        inv: &i,
        runner: &runner,
        now: SystemTime::now(),
    };
    let err = super::temp::TempScanner.scan(&cx).unwrap_err();
    assert_eq!(err.to_string(), "temp cleanup is not available as root");
    let fp = crate::safety::fingerprint(&env, Path::new("/tmp/old")).unwrap();
    let err = verify(&env, &cfg, Scope::UserTemp, Path::new("/tmp/old"), &fp).unwrap_err();
    assert_eq!(
        err.to_string(),
        "refusing: /tmp/old temp cleanup is not available as root"
    );
}

/// Through the real scan pipeline, size pass included: a stale dir item
/// still passes the execute-time recheck afterwards.
#[test]
fn temp_scan_pipeline_then_still_stale() {
    if crate::testing::running_as_root() {
        eprintln!("skipped: temp cleanup is not offered as root");
        return;
    }
    let t = FixtureTree::new();
    let mut old = vec![
        t.write("tmp/build-1/a/b/c.o", &[1u8; 3000]),
        t.write("tmp/build-1/d.o", &[1u8; 5000]),
        t.write("tmp/old.txt", b"x"),
    ];
    let tmp = t.dir.path().join("tmp");
    old.push(tmp.join("build-1/a/b"));
    old.push(tmp.join("build-1/a"));
    old.push(tmp.join("build-1"));
    for p in &old {
        age_nofollow(p, 30);
    }
    let env = t.env();
    let cfg = Config::default();
    let i = inv(vec![]);
    let runner = FakeRunner::new();
    let cx = ScanContext {
        env: &env,
        cfg: &cfg,
        inv: &i,
        runner: &runner,
        now: test_now(),
    };
    let rep = scan_all(&cx, &[Category::Temp]);
    assert_eq!(rep.categories.len(), 1);
    let cat = &rep.categories[0];
    assert!(cat.error.is_none(), "{:?}", cat.error);
    assert_eq!(
        paths(&cat.items),
        [PathBuf::from("/tmp/build-1"), PathBuf::from("/tmp/old.txt")]
    );
    assert!(cat.items[0].size.unwrap() >= 8000);
    for item in &cat.items {
        let p = item.path.as_ref().unwrap();
        super::temp::still_stale(&env, &cfg, p, test_now()).unwrap();
        verify(
            &env,
            &cfg,
            Scope::UserTemp,
            p,
            item.fingerprint.as_ref().unwrap(),
        )
        .unwrap();
    }
}

/// Archive extraction keeps old mtimes, but the ctime is when the file
/// landed: such entries are not old.
#[test]
fn old_mtime_fresh_ctime_not_listed() {
    if crate::testing::running_as_root() {
        eprintln!("skipped: temp cleanup is not offered as root");
        return;
    }
    let t = FixtureTree::new();
    let tmp = t.mkdir("tmp");
    let f = t.write("tmp/extracted.txt", b"x");
    let inner = t.write("tmp/unpacked/sub/a", b"x");
    let log = t.write("home/user/.local/state/app/old.log", b"x");
    for p in [
        &f,
        &inner,
        &tmp.join("unpacked/sub"),
        &tmp.join("unpacked"),
        &log,
    ] {
        // 30 days before the real time.
        age_nofollow(p, 130);
    }
    let cfg = Config::default();
    let r = FakeRunner::new();
    assert!(
        scan_real_now(&t, &cfg, &r, Category::Temp)
            .unwrap()
            .is_empty(),
        "fresh ctime keeps temp entries"
    );
    assert!(
        scan_real_now(&t, &cfg, &r, Category::Logs)
            .unwrap()
            .is_empty(),
        "fresh ctime keeps logs"
    );
    assert!(
        super::temp::still_stale(
            &t.env(),
            &cfg,
            Path::new("/tmp/extracted.txt"),
            SystemTime::now()
        )
        .is_err()
    );
    // The same entries 30 days later are old: the clock decides, not the
    // test setup.
    assert_eq!(scan_one(&t, &cfg, &r, Category::Temp).unwrap().len(), 2);
    assert_eq!(scan_one(&t, &cfg, &r, Category::Logs).unwrap().len(), 1);
}

#[test]
fn entry_stat_uses_later_of_mtime_and_ctime() {
    let t = FixtureTree::new();
    let f = t.write("tmp/f", b"x");
    age_nofollow(&f, 130);
    let m = fs::symlink_metadata(&f).unwrap();
    let st = super::temp::EntryStat::of(&m).unwrap();
    assert!(
        st.changed > m.modified().unwrap(),
        "ctime wins over an old mtime"
    );
    age_nofollow(&f, 0);
    let m = fs::symlink_metadata(&f).unwrap();
    let st = super::temp::EntryStat::of(&m).unwrap();
    assert_eq!(st.changed, m.modified().unwrap(), "a later mtime wins");
}

#[test]
fn cache_skips_sockets_fifos_and_devices() {
    use rustix::fs::{CWD, FileType, Mode, mknodat};
    let t = FixtureTree::new();
    t.write("home/user/.cache/plain/f", b"x");
    let cache = t.dir.path().join("home/user/.cache");
    mknodat(
        CWD,
        cache.join("fifo"),
        FileType::Fifo,
        Mode::from_raw_mode(0o600),
        0,
    )
    .unwrap();
    let _sock = std::os::unix::net::UnixListener::bind(cache.join("sock")).unwrap();
    let cfg = Config::default();
    let items = run(&t, &cfg, &inv(vec![]), Category::Cache);
    assert_eq!(paths(&items), [PathBuf::from("/home/user/.cache/plain")]);
}

/// `/tmp` must be owned by root (on the real system; the fixture user in
/// a fake root, see `Env::system_owner_uid`) and carry the sticky bit.
#[test]
fn temp_refuses_unsafe_tmp() {
    if crate::testing::running_as_root() {
        eprintln!("skipped: temp cleanup is not offered as root");
        return;
    }
    use std::os::unix::fs::PermissionsExt;
    let t = FixtureTree::new();
    let f = t.write("tmp/old", b"x");
    age_nofollow(&f, 30);
    let tmp = t.dir.path().join("tmp");
    let cfg = Config::default();
    let r = FakeRunner::new();
    let fp = crate::safety::fingerprint(&t.env(), Path::new("/tmp/old")).unwrap();
    // Sticky and owned as expected: listed and verifiable.
    fs::set_permissions(&tmp, fs::Permissions::from_mode(0o1777)).unwrap();
    assert_eq!(scan_one(&t, &cfg, &r, Category::Temp).unwrap().len(), 1);
    verify(&t.env(), &cfg, Scope::UserTemp, Path::new("/tmp/old"), &fp).unwrap();
    let unsafe_tmp = "unsafe /tmp: not root-owned with sticky bit";
    // No sticky bit.
    fs::set_permissions(&tmp, fs::Permissions::from_mode(0o777)).unwrap();
    let err = scan_one(&t, &cfg, &r, Category::Temp).unwrap_err();
    assert_eq!(err.to_string(), unsafe_tmp);
    let err = verify(&t.env(), &cfg, Scope::UserTemp, Path::new("/tmp/old"), &fp).unwrap_err();
    assert_eq!(
        err.to_string(),
        "refusing: /tmp is not root-owned with the sticky bit"
    );
    assert!(super::temp::still_stale(&t.env(), &cfg, Path::new("/tmp/old"), test_now()).is_err());
    // Sticky, but owned by someone other than expected.
    fs::set_permissions(&tmp, fs::Permissions::from_mode(0o1777)).unwrap();
    let mut env = t.env();
    env.system_owner_uid = env.uid.wrapping_add(1);
    let i = inv(vec![]);
    let cx = ScanContext {
        env: &env,
        cfg: &cfg,
        inv: &i,
        runner: &r,
        now: test_now(),
    };
    let err = super::temp::TempScanner.scan(&cx).unwrap_err();
    assert_eq!(err.to_string(), unsafe_tmp);
    assert!(verify(&env, &cfg, Scope::UserTemp, Path::new("/tmp/old"), &fp).is_err());
}

#[test]
fn real_system_tmp_must_be_root_owned() {
    if let Ok(env) = crate::env::Env::from_system() {
        assert_eq!(env.system_owner_uid, 0);
    }
    // A fake root's `/tmp` is the fixture user's.
    let t = FixtureTree::new();
    let env = t.env();
    assert_eq!(env.system_owner_uid, env.uid);
}

#[test]
fn temp_is_unavailable_not_an_error_as_root() {
    let t = FixtureTree::new();
    t.mkdir("tmp");
    let mut env = t.env();
    env.uid = 0;
    let runner = FakeRunner::new();
    let inv = Inventory {
        apps: vec![],
        statuses: vec![],
        errors: vec![],
        owned: Default::default(),
    };
    let cx = ScanContext {
        env: &env,
        cfg: &Config::default(),
        inv: &inv,
        runner: &runner,
        now: test_now(),
    };
    let rep = scan_all(&cx, &[Category::Temp]).categories.remove(0);
    assert_eq!(rep.unavailable.as_deref(), Some("not available as root"));
    assert!(rep.error.is_none() && rep.items.is_empty(), "{rep:?}");
}
