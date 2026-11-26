use super::*;

#[test]
fn home_itself_protected_exact_child_allowed() {
    let (_t, env, cfg) = setup();
    assert!(is_protected(&env, &cfg, &p("/home/user")));
    assert!(!is_allowed_user_target(&env, &cfg, &p("/home/user")));
    assert!(is_protected(&env, &cfg, &p("/home/user/.config")));
    assert!(!is_protected(&env, &cfg, &p("/home/user/.config/foo")));
    assert!(is_allowed_user_target(
        &env,
        &cfg,
        &p("/home/user/.config/foo")
    ));
}

#[test]
fn home_trash_scope_rules() {
    let (t, env, cfg) = setup();
    t.write("home/user/.local/share/Trash/files/x", b"x");
    t.write("home/user/.local/share/Trash/files/d/y", b"y");
    t.write("home/user/.local/share/Trash/info/x.trashinfo", b"i");
    t.write("home/user/.local/share/Trash/other", b"o");
    t.write("tmp/x", b"x");
    let ht = Scope::HomeTrash;
    let trash = "/home/user/.local/share/Trash";
    for s in [
        format!("{trash}/files/x"),
        format!("{trash}/files/d"),
        format!("{trash}/info/x.trashinfo"),
    ] {
        let target = p(&s);
        assert!(is_allowed_target(&env, &cfg, ht, &target), "{s}");
        let fp = fingerprint(&env, &target).unwrap();
        assert_eq!(verify_now(&env, &cfg, ht, &target), Ok(fp), "{s}");
        assert_eq!(verify(&env, &cfg, ht, &target, &fp), Ok(()), "{s}");
        assert!(
            !is_allowed_target(&env, &cfg, Scope::UserTemp, &target),
            "{s}"
        );
    }
    for s in [
        format!("{trash}/files/d/y"),
        trash.to_string(),
        format!("{trash}/files"),
        format!("{trash}/info"),
        format!("{trash}/other"),
        "/tmp/x".to_string(),
    ] {
        assert!(!is_allowed_target(&env, &cfg, ht, &p(&s)), "{s}");
        assert!(
            matches!(
                verify_now(&env, &cfg, ht, &p(&s)),
                Err(SafetyError::OutsideAllowedRoots(_) | SafetyError::Protected(_))
            ),
            "{s}"
        );
    }
}

#[test]
fn home_trash_outside_home_refused() {
    let (t, mut env, cfg) = setup();
    t.write("data/Trash/files/x", b"x");
    t.write("data/Trash/info/x.trashinfo", b"i");
    env.data_home = t.dir.path().join("data");
    for s in ["/data/Trash/files/x", "/data/Trash/info/x.trashinfo"] {
        let target = p(s);
        assert!(!is_allowed_target(&env, &cfg, Scope::HomeTrash, &target));
        assert!(matches!(
            verify_now(&env, &cfg, Scope::HomeTrash, &target),
            Err(SafetyError::OutsideAllowedRoots(_))
        ));
    }
}

#[test]
fn home_trash_requires_private_trash_dirs() {
    use std::os::unix::fs::PermissionsExt;
    let (t, env, cfg) = setup();
    t.write("home/user/.local/share/Trash/files/x", b"x");
    t.mkdir("home/user/.local/share/Trash/info");
    let target = p("/home/user/.local/share/Trash/files/x");
    let fp = fingerprint(&env, &target).unwrap();
    let ht = Scope::HomeTrash;
    assert_eq!(verify(&env, &cfg, ht, &target, &fp), Ok(()));
    let trash = t.dir.path().join("home/user/.local/share/Trash");
    std::fs::set_permissions(&trash, std::fs::Permissions::from_mode(0o777)).unwrap();
    let err = verify(&env, &cfg, ht, &target, &fp).unwrap_err();
    assert!(matches!(err, SafetyError::Ineligible(..)), "{err:?}");
    std::fs::set_permissions(&trash, std::fs::Permissions::from_mode(0o700)).unwrap();
    // Someone else's trash.
    let mut other = env.clone();
    other.uid = env.uid.wrapping_add(1);
    assert!(matches!(
        verify(&other, &cfg, ht, &target, &fp),
        Err(SafetyError::Ineligible(..))
    ));
    // A missing `info` dir fails closed too.
    std::fs::remove_dir(trash.join("info")).unwrap();
    assert!(verify(&env, &cfg, ht, &target, &fp).is_err());
    t.mkdir("home/user/.local/share/Trash/info");
    // `files` swapped for a symlink to an identical-looking tree.
    std::fs::rename(trash.join("files"), t.dir.path().join("real-files")).unwrap();
    t.symlink(
        "home/user/.local/share/Trash/files",
        "../../../../../real-files",
    );
    assert_eq!(
        verify(&env, &cfg, ht, &target, &fp),
        Err(SafetyError::SymlinkedParent(p(
            "~/.local/share/Trash/files"
        )))
    );
}

#[test]
fn ssh_subtree_protected() {
    let (_t, env, cfg) = setup();
    assert!(is_protected(&env, &cfg, &p("/home/user/.ssh/known_hosts")));
    assert!(is_protected(&env, &cfg, &p("/home/user/Documents/a/b")));
    assert!(!is_protected(&env, &cfg, &p("/home/user/.sshx")));
}

#[test]
fn ancestor_of_protected_is_protected() {
    let (_t, env, cfg) = setup();
    assert!(is_protected(&env, &cfg, &p("/home/user/.local")));
    assert!(is_protected(&env, &cfg, &p("/home/user/.local/share")));
    assert!(!is_protected(&env, &cfg, &p("/home/user/.local/share/foo")));
    assert!(is_protected(&env, &cfg, &p("/var/lib")));
}

#[test]
fn custom_xdg_crumbs_and_keyrings_protected() {
    let (_t, mut env, cfg) = setup();
    env.config_home = env.home.join("xdg/config");
    env.state_home = env.home.join("xdg/state");
    env.data_home = env.home.join("xdg/data");
    for s in [
        "/home/user/xdg/config/crumbs",
        "/home/user/xdg/config/crumbs/config.toml",
        "/home/user/xdg/state/crumbs/history.jsonl",
        "/home/user/xdg/data/keyrings/login.keyring",
        // The ~ forms stay protected too.
        "/home/user/.config/crumbs",
        "/home/user/.local/state/crumbs",
        "/home/user/.local/share/keyrings",
    ] {
        assert!(is_protected(&env, &cfg, &p(s)), "{s}");
        assert!(!is_allowed_user_target(&env, &cfg, &p(s)), "{s}");
    }
    assert!(is_allowed_user_target(
        &env,
        &cfg,
        &p("/home/user/xdg/config/other")
    ));
}

#[test]
fn system_paths_protected() {
    let (_t, env, cfg) = setup();
    for s in [
        "/",
        "/etc",
        "/opt",
        "/var",
        "/home",
        "/usr/bin/ls",
        "/boot/x",
    ] {
        assert!(is_protected(&env, &cfg, &p(s)), "{s}");
    }
    assert!(!is_protected(&env, &cfg, &p("/opt/foo")));
}

#[test]
fn system_scope_allows_core_files_only() {
    let (t, env, cfg) = setup();
    coredump_dir(&t);
    t.write("var/lib/systemd/coredump/core.a.1", b"x");
    t.write("var/lib/systemd/coredump/other", b"x");
    t.write("var/lib/systemd/coredump/sub/core.deep", b"x");
    t.write("var/lib/systemd/coredump/core.dir/inner", b"x");
    t.write("var/lib/systemd/other/core.x", b"x");
    let sys = Scope::System;
    let good = p("/var/lib/systemd/coredump/core.a.1");
    assert!(is_allowed_target(&env, &cfg, sys, &good));
    let fp = verify_now(&env, &cfg, sys, &good).unwrap();
    verify(&env, &cfg, sys, &good, &fp).unwrap();
    for s in [
        "/var/lib/systemd/coredump/other",
        "/var/lib/systemd/coredump/core.",
        "/var/lib/systemd/coredump",
        "/var/lib/systemd/coredump/sub/core.deep",
        "/var/lib/systemd/other/core.x",
        "/tmp/core.x",
        "/var/lib/systemd/coredump/../coredump/core.a.1",
        "/",
    ] {
        assert!(!is_allowed_target(&env, &cfg, sys, &p(s)), "{s}");
    }
    // A directory named core.* is not a regular file.
    let dir = p("/var/lib/systemd/coredump/core.dir");
    assert!(is_allowed_target(&env, &cfg, sys, &dir));
    assert!(matches!(
        verify_now(&env, &cfg, sys, &dir),
        Err(SafetyError::Ineligible(_, _))
    ));
    // Protected and excluded paths stay refused.
    let mut cfg = cfg;
    cfg.exclude.push("/var/lib/systemd/coredump/core.a*".into());
    assert!(!is_allowed_target(&env, &cfg, sys, &good));
    let mut cfg2 = Config::default();
    cfg2.protect.push(good.clone());
    assert!(!is_allowed_target(&env, &cfg2, sys, &good));
    // Changed since scanned.
    let cfg3 = Config::default();
    t.write("var/lib/systemd/coredump/core.a.1", b"y");
    std::fs::remove_file(t.dir.path().join("var/lib/systemd/coredump/core.a.1")).unwrap();
    t.write("var/lib/systemd/coredump/core.a.1", b"z");
    assert!(matches!(
        verify(&env, &cfg3, sys, &good, &fp),
        Err(SafetyError::Changed(_))
    ));
}

#[test]
fn system_scope_refuses_symlink_target() {
    let (t, env, cfg) = setup();
    coredump_dir(&t);
    t.write("var/lib/systemd/other", b"secret");
    t.symlink(
        "var/lib/systemd/coredump/core.link",
        "/var/lib/systemd/other",
    );
    let link = p("/var/lib/systemd/coredump/core.link");
    assert!(matches!(
        verify_now(&env, &cfg, Scope::System, &link),
        Err(SafetyError::Ineligible(_, _))
    ));
}

#[test]
fn system_scope_refuses_symlinked_chain_dir() {
    let (t, env, cfg) = setup();
    let real = t.mkdir("elsewhere/coredump");
    t.mkdir("var/lib/systemd");
    coredump_dir(&t);
    fs::remove_dir(t.dir.path().join("var/lib/systemd/coredump")).unwrap();
    std::os::unix::fs::symlink(&real, t.dir.path().join("var/lib/systemd/coredump")).unwrap();
    fs::write(real.join("core.x"), b"x").unwrap();
    assert!(matches!(
        verify_now(
            &env,
            &cfg,
            Scope::System,
            &p("/var/lib/systemd/coredump/core.x")
        ),
        Err(SafetyError::SymlinkedParent(_))
    ));
}

#[test]
fn system_scope_refuses_writable_parent() {
    use std::os::unix::fs::PermissionsExt;
    let (t, env, cfg) = setup();
    coredump_dir(&t);
    t.write("var/lib/systemd/coredump/core.x", b"x");
    let target = p("/var/lib/systemd/coredump/core.x");
    verify_now(&env, &cfg, Scope::System, &target).unwrap();
    // Any directory in the chain, the root included, group- or
    // other-writable refuses.
    for rel in [
        "",
        "var",
        "var/lib",
        "var/lib/systemd",
        "var/lib/systemd/coredump",
    ] {
        for bad in [0o775, 0o757] {
            let d = t.dir.path().join(rel);
            fs::set_permissions(&d, fs::Permissions::from_mode(bad)).unwrap();
            assert!(
                matches!(
                    verify_now(&env, &cfg, Scope::System, &target),
                    Err(SafetyError::Ineligible(_, why)) if why.contains("writable")
                ),
                "{rel} {bad:o}"
            );
            fs::set_permissions(&d, fs::Permissions::from_mode(0o755)).unwrap();
        }
    }
    verify_now(&env, &cfg, Scope::System, &target).unwrap();
}

#[test]
fn system_scope_refuses_non_root_parent() {
    let (t, mut env, cfg) = setup();
    coredump_dir(&t);
    t.write("var/lib/systemd/coredump/core.x", b"x");
    let target = p("/var/lib/systemd/coredump/core.x");
    verify_now(&env, &cfg, Scope::System, &target).unwrap();
    // The directories belong to the test user, not the expected owner.
    env.system_owner_uid = env.uid.wrapping_add(1);
    assert!(matches!(
        verify_now(&env, &cfg, Scope::System, &target),
        Err(SafetyError::Ineligible(_, why)) if why.contains("not owned by root")
    ));
}
