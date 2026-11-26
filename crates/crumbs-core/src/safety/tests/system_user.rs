use super::*;

#[test]
fn system_tree_allows_direct_children_only() {
    let (t, _env, cfg) = setup();
    let env = system_tree_env(&t);
    let st = Scope::SystemTree;
    for s in ["/opt/inky", "/etc/inky"] {
        assert!(is_allowed_target(&env, &cfg, st, &p(s)), "{s}");
    }
    for s in [
        "/opt/inky/sub",
        "/opt",
        "/etc",
        "/var/foo",
        "/etc/machine-id",
        "/etc/passwd-",
        "/etc/foo.pacsave",
        "/etc/foo.pacnew",
        "/etc/foo.bak",
        "/etc/foo.old",
        "/etc/.hidden",
        "/etc/../etc/inky",
    ] {
        assert!(!is_allowed_target(&env, &cfg, st, &p(s)), "{s}");
    }
    let mut cfg = cfg;
    cfg.exclude.push("/opt/inky".into());
    assert!(!is_allowed_target(&env, &cfg, st, &p("/opt/inky")));
}

#[test]
fn system_leftover_names() {
    for n in ETC_NEVER {
        assert!(!is_system_leftover_name(OsStr::new(n)), "{n}");
    }
    for n in [
        "subuid",
        "subgid",
        "kernel",
        "cmdline.d",
        "secureboot",
        "crypttab.initramfs",
        "machine-info",
        "resolv.conf",
    ] {
        assert!(!is_system_leftover_name(OsStr::new(n)), "{n}");
    }
    // Unowned on stock Debian/Ubuntu and Fedora.
    for n in [
        "nsswitch.conf",
        "shells",
        "motd",
        "opt",
        "securetty",
        "dconf",
        "issue",
        "timezone",
        "login.defs",
        "apt",
        "dpkg",
        "dnf",
        "yum.repos.d",
        "rpm",
        "sysconfig",
        "alternatives",
        "selinux",
        "sudo.conf",
    ] {
        assert!(!is_system_leftover_name(OsStr::new(n)), "{n}");
    }
    assert!(!is_system_leftover_name(OsStr::new("")));
    assert!(is_system_leftover_name(OsStr::new("inky")));
    assert!(is_system_leftover_name(OsStr::new("inky.conf")));
}

#[test]
fn system_tree_refuses_symlink_and_writable_parent() {
    use std::os::unix::fs::PermissionsExt;
    let (t, _env, cfg) = setup();
    let env = system_tree_env(&t);
    t.symlink("opt/x", "/etc");
    assert!(matches!(
        verify_now(&env, &cfg, Scope::SystemTree, &p("/opt/x")),
        Err(SafetyError::Ineligible(_, "is a symlink"))
    ));
    t.mkdir("opt/inky");
    t.write("opt/file", b"x");
    verify_now(&env, &cfg, Scope::SystemTree, &p("/opt/inky")).unwrap();
    verify_now(&env, &cfg, Scope::SystemTree, &p("/opt/file")).unwrap();
    fs::set_permissions(t.dir.path().join("opt"), fs::Permissions::from_mode(0o777)).unwrap();
    assert!(matches!(
        verify_now(&env, &cfg, Scope::SystemTree, &p("/opt/inky")),
        Err(SafetyError::Ineligible(_, "is writable by group or others"))
    ));
}

#[test]
fn system_tree_refuses_special_files() {
    let (t, _env, cfg) = setup();
    let env = system_tree_env(&t);
    let fifo = t.dir.path().join("opt/fifo");
    rustix::fs::mknodat(
        rustix::fs::CWD,
        &fifo,
        rustix::fs::FileType::Fifo,
        rustix::fs::Mode::from_raw_mode(0o600),
        0,
    )
    .unwrap();
    assert!(matches!(
        verify_now(&env, &cfg, Scope::SystemTree, &p("/opt/fifo")),
        Err(SafetyError::Ineligible(_, why)) if why.contains("regular file or directory")
    ));
}

#[test]
fn system_tree_verify_accepts_root_owned_chain() {
    let (t, _env, cfg) = setup();
    let env = system_tree_env(&t);
    t.mkdir("opt/inky");
    let target = p("/opt/inky");
    let st = Scope::SystemTree;
    let fp = verify_now(&env, &cfg, st, &target).unwrap();
    verify(&env, &cfg, st, &target, &fp).unwrap();
    // Keep the old directory alive so the new one has another inode.
    fs::rename(t.dir.path().join("opt/inky"), t.dir.path().join("opt/kept")).unwrap();
    t.mkdir("opt/inky");
    assert!(matches!(
        verify(&env, &cfg, st, &target, &fp),
        Err(SafetyError::Changed(_))
    ));
}

#[test]
fn system_tree_serde_is_kebab_case() {
    assert_eq!(
        serde_json::to_string(&Scope::SystemTree).unwrap(),
        "\"system-tree\""
    );
}

#[test]
fn system_scope_serde_is_kebab_case() {
    assert_eq!(serde_json::to_string(&Scope::System).unwrap(), "\"system\"");
}

#[test]
fn user_protect_added() {
    let (_t, env, mut cfg) = setup();
    let target = p("/home/user/.config/keepme/sub");
    assert!(is_allowed_user_target(&env, &cfg, &target));
    cfg.protect.push(p("/home/user/.config/keepme"));
    assert!(is_protected(&env, &cfg, &target));
    assert!(!is_allowed_user_target(&env, &cfg, &target));
}

#[test]
fn user_temp_scope_rules() {
    if crate::testing::running_as_root() {
        eprintln!("skipped: temp cleanup is not offered as root");
        return;
    }
    let (t, env, cfg) = setup();
    t.write("tmp/x", b"x");
    t.write("tmp/d/y", b"y");
    t.write("home/user/.cache/z", b"z");
    t.symlink("tmp/link", "/tmp/x");
    mkfifo(&t, "tmp/fifo");
    let ut = Scope::UserTemp;
    // A build tree directly in /var/tmp follows the same rules.
    t.write("var/tmp/build/x", b"x");
    for s in ["/tmp/x", "/tmp/d", "/var/tmp/build"] {
        let target = p(s);
        assert!(is_allowed_target(&env, &cfg, ut, &target), "{s}");
        let fp = fingerprint(&env, &target).unwrap();
        assert_eq!(verify_now(&env, &cfg, ut, &target), Ok(fp), "{s}");
        assert_eq!(verify(&env, &cfg, ut, &target, &fp), Ok(()), "{s}");
        // Home rules don't reach /tmp.
        assert!(!is_allowed_target(&env, &cfg, Scope::Home, &target), "{s}");
    }
    for s in [
        "/tmp",
        "/tmp/d/y",
        "/home/user/.cache/z",
        "/var/tmp",
        "/var/tmp/build/x",
        "/var/x",
        "tmp/x",
    ] {
        assert!(!is_allowed_target(&env, &cfg, ut, &p(s)), "{s}");
    }
    assert!(matches!(
        verify_now(&env, &cfg, ut, &p("/tmp/d/y")),
        Err(SafetyError::OutsideAllowedRoots(_))
    ));
    assert!(matches!(
        verify_now(&env, &cfg, ut, &p("/home/user/.cache/z")),
        Err(SafetyError::OutsideAllowedRoots(_))
    ));
    // Symlinks and fifos are refused even with a matching fingerprint.
    for s in ["/tmp/link", "/tmp/fifo"] {
        let target = p(s);
        let fp = fingerprint(&env, &target).unwrap();
        assert!(
            matches!(
                verify_now(&env, &cfg, ut, &target),
                Err(SafetyError::Ineligible(..))
            ),
            "{s}"
        );
        assert!(
            matches!(
                verify(&env, &cfg, ut, &target, &fp),
                Err(SafetyError::Ineligible(..))
            ),
            "{s}"
        );
    }
    // Someone else's file.
    let mut other = env.clone();
    other.uid = env.uid.wrapping_add(1);
    let err = verify_now(&other, &cfg, ut, &p("/tmp/x")).unwrap_err();
    assert_eq!(err.to_string(), "refusing: /tmp/x is not owned by you");
    assert!(matches!(
        verify_now(&env, &cfg, ut, &p("/tmp/gone")),
        Err(SafetyError::Missing(_))
    ));
}

#[test]
fn user_temp_refuses_symlinked_tmp() {
    let (t, env, cfg) = setup();
    t.write("elsewhere/x", b"x");
    t.symlink("tmp", "elsewhere");
    let target = p("/tmp/x");
    let fp = fingerprint(&env, &target).unwrap();
    assert_eq!(
        verify(&env, &cfg, Scope::UserTemp, &target, &fp),
        Err(SafetyError::SymlinkedParent(p("/tmp")))
    );
}

#[test]
fn exclude_glob_blocks_target() {
    let (_t, env, mut cfg) = setup();
    cfg.exclude.push("/home/user/.config/*-keep".into());
    assert!(!is_allowed_user_target(
        &env,
        &cfg,
        &p("/home/user/.config/a-keep/x")
    ));
    assert!(is_allowed_user_target(
        &env,
        &cfg,
        &p("/home/user/.config/a-drop")
    ));
}

#[test]
fn exclude_protects_ancestors_of_its_literal_prefix() {
    let (_t, env, mut cfg) = setup();
    cfg.exclude.push("/home/user/.config/Foobar/keep".into());
    cfg.exclude
        .push("/home/user/.local/share/Baz/sub/*.db".into());
    for s in [
        "/home/user/.config/Foobar/keep",
        "/home/user/.config/Foobar/keep/important",
        "/home/user/.config/Foobar",
        "/home/user/.local/share/Baz/sub",
        "/home/user/.local/share/Baz",
        "/home/user/.local/share/Baz/sub/x.db",
    ] {
        assert!(!is_allowed_user_target(&env, &cfg, &p(s)), "{s}");
    }
    for s in [
        "/home/user/.config/Foobar/other",
        "/home/user/.config/Foobarx",
        "/home/user/.local/share/Baz/sub/x.txt",
        "/home/user/.local/share/Baz/other",
    ] {
        assert!(is_allowed_user_target(&env, &cfg, &p(s)), "{s}");
    }
}

#[test]
fn exclude_wildcard_shields_parent() {
    let (_t, env, mut cfg) = setup();
    cfg.exclude.push("/home/user/.config/*/keep".into());
    for s in [
        "/home/user/.config/Foobar",
        "/home/user/.config",
        "/home/user/.config/Foobar/keep",
    ] {
        assert!(!is_allowed_user_target(&env, &cfg, &p(s)), "{s}");
    }
    assert!(is_allowed_user_target(
        &env,
        &cfg,
        &p("/home/user/.cache/x")
    ));
}

#[test]
fn outside_home_not_allowed() {
    let (_t, env, cfg) = setup();
    assert!(!is_allowed_user_target(&env, &cfg, &p("/opt/foo")));
    assert!(!is_allowed_user_target(&env, &cfg, &p("/tmp/x")));
    assert!(!is_allowed_user_target(&env, &cfg, &p("/home/username/x")));
}
