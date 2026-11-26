use super::*;

#[test]
fn measurement_counts_a_folder_missing_both_times_as_empty() {
    // dnf: only one of /var/cache/dnf and /var/cache/libdnf5 exists.
    let t = FixtureTree::new();
    logging_program(&t, "dnf", &t.dir.path().join("l.log"), "", 0);
    let old = t.write("var/cache/libdnf5/repo/packages/a.rpm", &[1u8; 12_000]);
    let env = t.env();
    let gone =
        crate::size::disk_usage(&env, Path::new("/var/cache/libdnf5/repo/packages/a.rpm")).bytes;
    let report = execute_full(
        &env,
        &Config::default(),
        &plan_of(vec![priv_cmd("dnf", &["clean", "packages"])]),
        &FakeRunner::new(),
        &RemovingRunner {
            remove: vec![old],
            out: b"@@crumbs 0 0\n".to_vec(),
        },
        Some(&esc()),
        &no_post_check,
    );
    assert!(report.results[0].ok, "{:?}", report.results[0]);
    assert_eq!(report.results[0].freed, Some(gone));
}

#[test]
fn commands_sharing_a_measured_folder_are_not_measured() {
    // Both paccache steps shrink the package cache in one batch: neither
    // can be credited with the whole drop, so both keep their estimates.
    let t = FixtureTree::new();
    logging_program(&t, "paccache", &t.dir.path().join("l.log"), "", 0);
    let a = t.write("var/cache/pacman/pkg/a-1.pkg.tar.zst", &[1u8; 20_000]);
    let b = t.write("var/cache/pacman/pkg/b-1.pkg.tar.zst", &[1u8; 20_000]);
    let env = t.env();
    let mut first = priv_cmd("paccache", &["-ruk0"]);
    first.size = 1000;
    let mut second = priv_cmd("paccache", &["-rk2"]);
    second.size = 2000;
    let report = execute_full(
        &env,
        &Config::default(),
        &plan_of(vec![first, second]),
        &FakeRunner::new(),
        &RemovingRunner {
            remove: vec![a, b],
            out: b"@@crumbs 0 0\n@@crumbs 1 0\n".to_vec(),
        },
        Some(&esc()),
        &no_post_check,
    );
    assert!(report.results.iter().all(|r| r.ok), "{:?}", report.results);
    assert_eq!(report.results[0].freed, None);
    assert_eq!(report.results[1].freed, None);
    assert_eq!(report.freed, 3000);
}

#[test]
fn an_unreadable_measured_folder_is_not_measured() {
    // Docker's /var/lib/docker cannot be read by the user: before and after
    // are both empty, which says nothing about what the prune freed.
    if crate::testing::running_as_root() {
        eprintln!("skipped: root reads every folder");
        return;
    }
    use std::os::unix::fs::PermissionsExt;
    let t = FixtureTree::new();
    logging_program(&t, "docker", &t.dir.path().join("l.log"), "", 0);
    let locked = t.mkdir("var/lib/docker");
    t.write("var/lib/docker/overlay2/x", b"x");
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
    let env = t.env();
    let mut plan = command_plan("docker", &["image", "prune", "-f"]);
    plan.actions[0].size = 5000;
    let report = execute_with(&env, &Config::default(), &plan, &RemovingCommand(vec![]));
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(report.results[0].ok, "{:?}", report.results[0]);
    assert_eq!(report.results[0].freed, None);
    assert_eq!(report.freed, 5000);
}

#[test]
fn a_build_tree_in_tmp_keeps_the_temp_rechecks() {
    // Only /var/tmp children are re-checked as build trees; in /tmp a
    // pamac-build-* entry still has to be an old temporary file.
    if crate::testing::running_as_root() {
        eprintln!("skipped: temp cleanup is not offered as root");
        return;
    }
    let t = FixtureTree::new();
    let f = t.write("tmp/pamac-build-user/a/x", b"fresh");
    age_days(&f, 0);
    let env = temp_env(&t);
    let mut item = planned(&env, delete("/tmp/pamac-build-user"));
    item.scope = Scope::UserTemp;
    let report = execute(&env, &Config::default(), &plan_of(vec![item]));
    assert!(!report.results[0].ok);
    assert_eq!(
        report.results[0].error.as_deref(),
        Some("refusing: /tmp/pamac-build-user is no longer an old temporary file")
    );
}

#[test]
fn a_tree_with_read_only_folders_is_deleted() {
    if crate::testing::running_as_root() {
        eprintln!("skipped: user-temp targets are refused as root");
        return;
    }
    // Go's module cache makes its folders read-only (0555): what is in
    // them cannot be removed until they are writable again.
    use std::os::unix::fs::PermissionsExt;
    let t = FixtureTree::new();
    t.write("var/tmp/pamac-build-user/yay/go/pkg/mod/m@v1/a.go", b"x");
    let env = t.env();
    for d in ["go/pkg/mod/m@v1", "go/pkg/mod"] {
        let p = env.sys(&format!("/var/tmp/pamac-build-user/yay/{d}"));
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o555)).unwrap();
    }
    let mut item = planned(&env, delete("/var/tmp/pamac-build-user"));
    item.scope = Scope::UserTemp;
    let report = execute(&env, &Config::default(), &plan_of(vec![item]));
    assert!(report.results[0].ok, "{:?}", report.results[0]);
    assert!(!env.sys("/var/tmp/pamac-build-user").exists());
}

#[test]
fn making_folders_writable_never_leaves_the_tree() {
    // A link in the tree to a read-only folder of yours elsewhere: the
    // retry after a permission error leaves that folder as it was.
    use std::os::unix::fs::PermissionsExt;
    if crate::testing::running_as_root() {
        eprintln!("skipped: root ignores directory permissions");
        return;
    }
    let t = FixtureTree::new();
    t.write("home/user/keep/readonly/file", b"k");
    let outside = t.dir.path().join("home/user/keep/readonly");
    std::fs::set_permissions(&outside, std::fs::Permissions::from_mode(0o555)).unwrap();
    t.write("var/tmp/pamac-build-user/go/mod/x/a", b"a");
    t.symlink(
        "var/tmp/pamac-build-user/go/link",
        "../../../../home/user/keep/readonly",
    );
    let env = t.env();
    let ro = env.sys("/var/tmp/pamac-build-user/go/mod/x");
    std::fs::set_permissions(&ro, std::fs::Permissions::from_mode(0o555)).unwrap();
    let mut item = planned(&env, delete("/var/tmp/pamac-build-user"));
    item.scope = Scope::UserTemp;
    let report = execute(&env, &Config::default(), &plan_of(vec![item]));
    assert!(report.results[0].ok, "{:?}", report.results[0]);
    let mode = std::fs::metadata(&outside).unwrap().permissions().mode() & 0o777;
    std::fs::set_permissions(&outside, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(mode, 0o555, "a folder outside the tree was made writable");
}
