use super::*;

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

#[test]
fn aur_build_not_available_as_root() {
    // As root, the safety checks refuse every user-temp target, so the
    // trees are not offered at all (as for `temp`).
    let t = FixtureTree::new();
    t.write("var/tmp/pamac-build-user/brave/src/x", b"x");
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
    let s = super::super::aur::AurBuildScanner;
    assert_eq!(s.unavailable(&cx).as_deref(), Some("not available as root"));
    assert!(s.scan(&cx).unwrap().is_empty());
}

#[test]
fn aur_build_lists_pamac_build_trees() {
    if crate::testing::running_as_root() {
        eprintln!("skipped: aur-build is not offered as root");
        return;
    }
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
    if crate::testing::running_as_root() {
        eprintln!("skipped: aur-build is not offered as root");
        return;
    }
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
    if crate::testing::running_as_root() {
        eprintln!("skipped: aur-build is not offered as root");
        return;
    }
    let t = FixtureTree::new();
    t.write("var/tmp/pamac-build-user/brave/src/x", b"x");
    t.symlink("proc/77/cwd", "/var/tmp/pamac-build-user/brave/src");
    t.write("proc/77/comm", b"makepkg\n");
    let items = run(&t, &Config::default(), &inv(vec![]), Category::AurBuild);
    assert!(!items[0].preselected);
    assert_eq!(items[0].note.as_deref(), Some("in use by makepkg (pid 77)"));
}

#[test]
fn aur_build_with_a_file_open_inside_not_preselected() {
    if crate::testing::running_as_root() {
        eprintln!("skipped: aur-build is not offered as root");
        return;
    }
    // pamac writing into the tree from elsewhere: an open file counts.
    let t = FixtureTree::new();
    t.write("var/tmp/pamac-build-user/brave/src/x", b"x");
    t.symlink("proc/66/fd/3", "/var/tmp/pamac-build-user/brave/src/x");
    t.write("proc/66/comm", b"pamac\n");
    let items = run(&t, &Config::default(), &inv(vec![]), Category::AurBuild);
    assert!(!items[0].preselected);
    assert_eq!(items[0].note.as_deref(), Some("in use by pamac (pid 66)"));
}
