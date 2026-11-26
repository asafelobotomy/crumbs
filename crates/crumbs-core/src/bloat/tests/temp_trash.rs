use super::*;

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
    let err = super::super::temp::TempScanner.scan(&cx).unwrap_err();
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
        super::super::temp::still_stale(&env, &cfg, p, test_now()).unwrap();
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
    assert!(
        super::super::temp::still_stale(&t.env(), &cfg, Path::new("/tmp/old"), test_now()).is_err()
    );
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
    let err = super::super::temp::TempScanner.scan(&cx).unwrap_err();
    assert_eq!(err.to_string(), unsafe_tmp);
    assert!(verify(&env, &cfg, Scope::UserTemp, Path::new("/tmp/old"), &fp).is_err());
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
        super::super::temp::still_stale(
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
    let st = super::super::temp::EntryStat::of(&m).unwrap();
    assert!(
        st.changed > m.modified().unwrap(),
        "ctime wins over an old mtime"
    );
    age_nofollow(&f, 0);
    let m = fs::symlink_metadata(&f).unwrap();
    let st = super::super::temp::EntryStat::of(&m).unwrap();
    assert_eq!(st.changed, m.modified().unwrap(), "a later mtime wins");
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
