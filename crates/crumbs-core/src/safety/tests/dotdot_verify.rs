use super::*;

#[test]
fn dotdot_and_relative_fail_closed() {
    let (_t, env, cfg) = setup();
    assert!(is_protected(
        &env,
        &cfg,
        &p("/home/user/.config/foo/../../.ssh")
    ));
    assert!(!is_allowed_user_target(
        &env,
        &cfg,
        &p("/home/user/.config/foo/../bar")
    ));
    assert!(!is_allowed_user_target(&env, &cfg, &p("foo")));
    // `.` components are dropped, so these compare normally.
    assert!(is_protected(&env, &cfg, &p("/home/user/./.ssh/x")));
    assert!(is_allowed_user_target(
        &env,
        &cfg,
        &p("/home/user/./.config/foo")
    ));
}

#[test]
fn verify_ok_when_unchanged() {
    let (t, env, cfg) = setup();
    t.mkdir("home/user/.config/foo");
    let target = p("/home/user/.config/foo");
    let fp = fingerprint(&env, &target).unwrap();
    assert!(!fp.is_symlink);
    assert_eq!(verify(&env, &cfg, Scope::Home, &target, &fp), Ok(()));
}

#[test]
fn verify_detects_swap() {
    let (t, env, cfg) = setup();
    t.mkdir("home/user/.config/foo");
    t.mkdir("home/user/Documents");
    let target = p("/home/user/.config/foo");
    let fp = fingerprint(&env, &target).unwrap();
    std::fs::remove_dir(t.dir.path().join("home/user/.config/foo")).unwrap();
    t.symlink("home/user/.config/foo", "/home/user/Documents");
    assert!(matches!(
        verify(&env, &cfg, Scope::Home, &target, &fp),
        Err(SafetyError::Changed(_))
    ));
}

#[test]
fn verify_detects_symlinked_parent() {
    let (t, env, cfg) = setup();
    t.write("home/user/.config/foo/bar", b"x");
    t.write("home/user/elsewhere/foo/bar", b"x");
    let target = p("/home/user/.config/foo/bar");
    let fp = fingerprint(&env, &target).unwrap();
    std::fs::remove_dir_all(t.dir.path().join("home/user/.config")).unwrap();
    t.symlink("home/user/.config", "/home/user/elsewhere");
    let err = verify(&env, &cfg, Scope::Home, &target, &fp).unwrap_err();
    assert_eq!(
        err,
        SafetyError::SymlinkedParent(PathBuf::from("~/.config"))
    );
    assert_eq!(err.to_string(), "refusing: ~/.config is a symlink");
}

#[test]
fn verify_now_checks_location_and_existence() {
    let (t, env, cfg) = setup();
    t.write("home/user/Applications/X.AppImage", b"x");
    t.write("opt/apps/Y.AppImage", b"y");
    let ok = p("/home/user/Applications/X.AppImage");
    assert_eq!(
        verify_now(&env, &cfg, Scope::Home, &ok),
        Ok(fingerprint(&env, &ok).unwrap())
    );
    assert!(matches!(
        verify_now(&env, &cfg, Scope::Home, &p("/opt/apps/Y.AppImage")),
        Err(SafetyError::OutsideAllowedRoots(_))
    ));
    assert!(matches!(
        verify_now(
            &env,
            &cfg,
            Scope::Home,
            &p("/home/user/Applications/Z.AppImage")
        ),
        Err(SafetyError::Missing(_))
    ));
    std::fs::rename(
        t.dir.path().join("home/user/Applications"),
        t.dir.path().join("home/user/real"),
    )
    .unwrap();
    t.symlink("home/user/Applications", "real");
    assert_eq!(
        verify_now(&env, &cfg, Scope::Home, &ok),
        Err(SafetyError::SymlinkedParent(p("~/Applications")))
    );
}

#[test]
fn verify_missing() {
    let (t, env, cfg) = setup();
    t.write("home/user/.config/foo/bar", b"x");
    let target = p("/home/user/.config/foo/bar");
    let fp = fingerprint(&env, &target).unwrap();
    std::fs::remove_file(t.dir.path().join("home/user/.config/foo/bar")).unwrap();
    assert!(matches!(
        verify(&env, &cfg, Scope::Home, &target, &fp),
        Err(SafetyError::Missing(_))
    ));
}

#[test]
fn verify_refuses_protected_and_outside() {
    let (t, env, cfg) = setup();
    t.write("home/user/.ssh/id", b"k");
    t.write("opt/foo", b"x");
    let ssh = p("/home/user/.ssh/id");
    let fp = fingerprint(&env, &ssh).unwrap();
    assert!(matches!(
        verify(&env, &cfg, Scope::Home, &ssh, &fp),
        Err(SafetyError::Protected(_))
    ));
    let out = p("/tmp/x");
    t.write("tmp/x", b"x");
    let fp = fingerprint(&env, &out).unwrap();
    assert!(matches!(
        verify(&env, &cfg, Scope::Home, &out, &fp),
        Err(SafetyError::OutsideAllowedRoots(_))
    ));
}

#[test]
fn verify_refuses_non_canonical_symlink_spellings() {
    let (t, env, cfg) = setup();
    t.mkdir("home/user/Documents");
    t.symlink("home/user/.config/foo", "/home/user/Documents");
    let link = p("/home/user/.config/foo");
    let fp = fingerprint(&env, &link).unwrap();
    assert!(fp.is_symlink);
    assert_eq!(verify(&env, &cfg, Scope::Home, &link, &fp), Ok(()));
    for s in [
        "/home/user/.config/foo/",
        "/home/user/.config/foo/.",
        "/home/user/.config//foo",
    ] {
        assert!(
            matches!(
                verify(&env, &cfg, Scope::Home, &p(s), &fp),
                Err(SafetyError::NonCanonical(_))
            ),
            "{s}"
        );
    }
}

#[test]
fn fingerprint_refuses_non_canonical() {
    let (t, env, _cfg) = setup();
    t.mkdir("home/user/.config/foo");
    for s in [
        "/home/user/.config/foo/",
        "/home/user/.config/foo/.",
        "/home/user/.config/../.config/foo",
    ] {
        assert!(fingerprint(&env, &p(s)).is_err(), "{s}");
    }
    assert!(fingerprint(&env, &p("/home/user/.config/foo")).is_ok());
}

#[test]
fn fingerprint_detects_inode_reuse() {
    // Same dev, ino and kind but a different ctime: a file replaced and
    // handed the old inode.
    let t = FixtureTree::new();
    t.write("/home/user/.config/foo/bar", b"x");
    let env = t.env();
    let p = std::path::Path::new("/home/user/.config/foo/bar");
    let old = fingerprint(&env, p).unwrap();
    let mut stale = old;
    stale.ctime_nsec += 1;
    assert_ne!(old, stale);
    let cfg = Config::default();
    assert!(verify(&env, &cfg, Scope::Home, p, &old).is_ok());
    assert_eq!(
        verify(&env, &cfg, Scope::Home, p, &stale),
        Err(SafetyError::Changed("~/.config/foo/bar".into()))
    );
}

#[test]
fn protect_applies_in_every_scope() {
    let (t, env, mut cfg) = setup();
    t.write("tmp/keep", b"k");
    t.write("tmp/excluded", b"e");
    t.write("home/user/.local/share/Trash/files/keep", b"k");
    t.mkdir("home/user/.local/share/Trash/info");
    t.write("home/user/.config/keep", b"k");
    cfg.protect.push(p("/tmp/keep"));
    cfg.protect
        .push(p("/home/user/.local/share/Trash/files/keep"));
    cfg.protect.push(p("/home/user/.config/keep"));
    cfg.exclude.push("/tmp/exclu*".into());
    for (scope, s) in [
        (Scope::UserTemp, "/tmp/keep"),
        (Scope::HomeTrash, "/home/user/.local/share/Trash/files/keep"),
        (Scope::Home, "/home/user/.config/keep"),
    ] {
        let target = p(s);
        assert!(is_protected(&env, &cfg, &target), "{s}");
        assert!(!is_allowed_target(&env, &cfg, scope, &target), "{s}");
        let fp = fingerprint(&env, &target).unwrap();
        assert!(
            matches!(
                verify(&env, &cfg, scope, &target, &fp),
                Err(SafetyError::Protected(_))
            ),
            "{s}"
        );
    }
    // Built-in protected paths too: `/` is never a temp child.
    for scope in [Scope::Home, Scope::UserTemp, Scope::HomeTrash] {
        assert!(!is_allowed_target(&env, &cfg, scope, &p("/")));
    }
    let excluded = p("/tmp/excluded");
    assert!(!is_allowed_target(&env, &cfg, Scope::UserTemp, &excluded));
    assert!(matches!(
        verify_now(&env, &cfg, Scope::UserTemp, &excluded),
        Err(SafetyError::OutsideAllowedRoots(_))
    ));
}

#[test]
fn scope_serde_is_kebab_case_and_defaults_to_home() {
    assert_eq!(Scope::default(), Scope::Home);
    for (scope, text) in [
        (Scope::Home, "\"home\""),
        (Scope::UserTemp, "\"user-temp\""),
        (Scope::HomeTrash, "\"home-trash\""),
    ] {
        assert_eq!(serde_json::to_string(&scope).unwrap(), text);
        assert_eq!(serde_json::from_str::<Scope>(text).unwrap(), scope);
    }
}

#[test]
fn is_allowed_target_home_matches_user_target() {
    let (_t, env, cfg) = setup();
    for s in [
        "/home/user",
        "/home/user/.config/foo",
        "/tmp/x",
        "/home/user/.ssh/x",
    ] {
        assert_eq!(
            is_allowed_target(&env, &cfg, Scope::Home, &p(s)),
            is_allowed_user_target(&env, &cfg, &p(s)),
            "{s}"
        );
    }
}

#[test]
fn directory_gaining_an_entry_still_verifies() {
    let t = FixtureTree::new();
    t.write("/home/user/.config/foo/bar", b"x");
    let env = t.env();
    let d = std::path::Path::new("/home/user/.config/foo");
    let fp = fingerprint(&env, d).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(20));
    t.write("/home/user/.config/foo/new", b"y");
    assert!(verify(&env, &Config::default(), Scope::Home, d, &fp).is_ok());
}
