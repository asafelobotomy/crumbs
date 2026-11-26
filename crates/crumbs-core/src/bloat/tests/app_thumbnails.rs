use super::*;

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
