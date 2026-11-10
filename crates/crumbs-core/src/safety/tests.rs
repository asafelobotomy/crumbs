use super::*;
use crate::testing::FixtureTree;

fn setup() -> (FixtureTree, Env, Config) {
    let t = FixtureTree::new();
    let env = t.env();
    (t, env, Config::default())
}

fn p(s: &str) -> PathBuf {
    PathBuf::from(s)
}

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
fn user_protect_added() {
    let (_t, env, mut cfg) = setup();
    let target = p("/home/user/.config/keepme/sub");
    assert!(is_allowed_user_target(&env, &cfg, &target));
    cfg.protect.push(p("/home/user/.config/keepme"));
    assert!(is_protected(&env, &cfg, &target));
    assert!(!is_allowed_user_target(&env, &cfg, &target));
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

fn mkfifo(t: &FixtureTree, rel: &str) {
    use rustix::fs::{CWD, FileType, Mode, mknodat};
    mknodat(
        CWD,
        t.dir.path().join(rel),
        FileType::Fifo,
        Mode::from_raw_mode(0o600),
        0,
    )
    .unwrap();
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

/// A fixture `/var/lib/systemd/coredump` with every directory from the
/// root down set to 0755, so the root-owned chain check passes (the
/// fixture user stands in for root, see `Env::system_owner_uid`).
fn coredump_dir(t: &FixtureTree) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let dir = t.mkdir("var/lib/systemd/coredump");
    let mut cur = t.dir.path().to_path_buf();
    fs::set_permissions(&cur, fs::Permissions::from_mode(0o755)).unwrap();
    for c in ["var", "lib", "systemd", "coredump"] {
        cur.push(c);
        fs::set_permissions(&cur, fs::Permissions::from_mode(0o755)).unwrap();
    }
    dir
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

/// `/`, `/etc` and `/opt` as 0755 directories, so the root-owned chain
/// check passes (the fixture user stands in for root).
fn system_tree_env(t: &FixtureTree) -> Env {
    use std::os::unix::fs::PermissionsExt;
    t.mkdir("etc");
    t.mkdir("opt");
    for rel in ["", "etc", "opt"] {
        fs::set_permissions(t.dir.path().join(rel), fs::Permissions::from_mode(0o755)).unwrap();
    }
    let mut env = t.env();
    env.set_system_owner_uid(env.uid);
    env
}

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
