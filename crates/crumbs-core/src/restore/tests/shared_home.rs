use super::*;

#[test]
fn shared_dot_trash_needs_sticky() {
    let (t, env) = topdir_fixture();
    let cfg = Config::default();
    let uid = env.uid;
    t.mkdir(&format!("home/.Trash/{uid}/files"));
    t.mkdir(&format!("home/.Trash/{uid}/info"));
    mode(&t, &format!("home/.Trash/{uid}"), 0o700);
    let e = entry_in(&env, &format!("/home/.Trash/{uid}"));
    // `.Trash` without the sticky bit.
    mode(&t, "home/.Trash", 0o777);
    assert!(valid_trash(&env, &cfg, &e).is_none());
    mode(&t, "home/.Trash", 0o1777);
    assert!(valid_trash(&env, &cfg, &e).is_some());
    // A symlinked `.Trash`.
    let real = t.dir.path().join("home/.Trash");
    fs::rename(&real, t.dir.path().join("home/.Trash-real")).unwrap();
    std::os::unix::fs::symlink(t.dir.path().join("home/.Trash-real"), &real).unwrap();
    assert!(valid_trash(&env, &cfg, &e).is_none());
}

#[test]
fn home_trash_outside_home_not_restorable() {
    let t = FixtureTree::new();
    t.write("home/user/.config/a/x", b"A");
    let env = t.env();
    run(&env, vec![trash("/home/user/.config/a")]);
    let mut env2 = env.clone();
    env2.data_home = t.dir.path().join("data");
    let e = entry_in(&env, "/data/Trash");
    assert!(valid_trash(&env2, &Config::default(), &e).is_none());
}

#[test]
fn bad_topdir_root_gets_a_permissions_message() {
    let (t, env) = topdir_fixture();
    let cfg = Config::default();
    let uid = env.uid;
    let e = entry_in(&env, &format!("/home/.Trash-{uid}"));
    mode(&t, &format!("home/.Trash-{uid}"), 0o755);
    let want = format!(
        "trash directory /home/.Trash-{uid} has unsafe permissions or owner \
             (needs a private 0700 directory owned by you)"
    );
    assert_eq!(restore_one(&env, &cfg, &e, true, "1"), Err(want));
    // A home trash outside the home gets it too.
    let mut env2 = env.clone();
    env2.data_home = t.dir.path().join("data");
    let e = entry_in(&env, "/data/Trash");
    assert_eq!(
        restore_one(&env2, &cfg, &e, true, "1"),
        Err(
            "trash directory /data/Trash has unsafe permissions or owner \
                 (needs a private 0700 directory owned by you)"
                .to_string()
        )
    );
    // Other invalid entries keep the generic message.
    let mut e = entry_in(&env, &format!("/home/.Trash-{uid}"));
    e.path = "/etc/passwd".into();
    assert_eq!(
        restore_one(&env, &cfg, &e, true, "1"),
        Err(INVALID_ENTRY.to_string())
    );
}
