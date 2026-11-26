use super::*;

#[test]
fn execute_many_denied_fails_every_plan() {
    let t = FixtureTree::new();
    let log = t.dir.path().join("calls.log");
    logging_program(&t, "pacman", &log, "", 0);
    let env = t.env();
    let plans = [
        app_plan("pacman:foo", vec![pkg_remove(&["foo"], &["1-1"])]),
        // A plan with nothing privileged is not flagged.
        app_plan("x", Vec::new()),
        app_plan("pacman:bar", vec![priv_cmd("pacman", &["-Rn", "bar"])]),
    ];
    let pr = DeniedRunner::default();
    let reports = execute_many(
        &env,
        &Config::default(),
        &plans,
        &FakeRunner::new(),
        &pr,
        Some(&esc()),
        &|_, _, names| Ok(names.to_vec()),
    );
    assert_eq!(*pr.calls.lock().unwrap(), 1);
    assert!(reports[0].escalation_failed && reports[2].escalation_failed);
    assert!(!reports[1].escalation_failed);
    assert_eq!(
        reports[0].results[0].error.as_deref(),
        Some("privilege escalation was cancelled or denied; still installed: foo")
    );
    assert_eq!(
        reports[2].results[0].error.as_deref(),
        Some(ESCALATION_DENIED)
    );
}

#[test]
fn execute_refuses_a_build_tree_in_use() {
    if crate::testing::running_as_root() {
        eprintln!("skipped: user-temp targets are refused as root");
        return;
    }
    // A /var/tmp build tree is deleted only while no process works in it.
    let t = FixtureTree::new();
    t.write("var/tmp/pamac-build-user/brave/src/x", b"x");
    t.write("var/tmp/pamac-build-other/a/x", b"x");
    t.symlink("proc/77/cwd", "/var/tmp/pamac-build-user/brave/src");
    t.write("proc/77/comm", b"makepkg\n");
    let env = t.env();
    let mut busy = planned(&env, delete("/var/tmp/pamac-build-user"));
    busy.scope = Scope::UserTemp;
    let mut idle = planned(&env, delete("/var/tmp/pamac-build-other"));
    idle.scope = Scope::UserTemp;
    let report = execute(&env, &Config::default(), &plan_of(vec![busy, idle]));
    assert!(!report.results[0].ok);
    assert_eq!(
        report.results[0].error.as_deref(),
        Some("refusing: /var/tmp/pamac-build-user is in use by makepkg (pid 77)")
    );
    assert!(env.sys("/var/tmp/pamac-build-user/brave/src/x").exists());
    assert!(report.results[1].ok, "{:?}", report.results[1]);
    assert!(!env.sys("/var/tmp/pamac-build-other").exists());
}

#[test]
fn execute_refuses_a_build_tree_with_a_file_open() {
    if crate::testing::running_as_root() {
        eprintln!("skipped: user-temp targets are refused as root");
        return;
    }
    let t = FixtureTree::new();
    t.write("var/tmp/pamac-build-user/brave/src/x", b"x");
    t.symlink("proc/66/fd/3", "/var/tmp/pamac-build-user/brave/src/x");
    t.write("proc/66/comm", b"pamac\n");
    let env = t.env();
    let mut busy = planned(&env, delete("/var/tmp/pamac-build-user"));
    busy.scope = Scope::UserTemp;
    let report = execute(&env, &Config::default(), &plan_of(vec![busy]));
    assert_eq!(
        report.results[0].error.as_deref(),
        Some("refusing: /var/tmp/pamac-build-user is in use by pamac (pid 66)")
    );
    assert!(env.sys("/var/tmp/pamac-build-user/brave/src/x").exists());
}

#[test]
fn edit_file_backup_and_filter() {
    let t = FixtureTree::new();
    let real = t.write("home/user/.config/mimeapps.list", MIME);
    fs::set_permissions(&real, fs::Permissions::from_mode(0o640)).unwrap();
    let env = t.env();
    let plan = plan_of(vec![planned(&env, mime_edit())]);
    let report = execute(&env, &Config::default(), &plan);
    assert!(report.results[0].ok, "{report:?}");
    assert_eq!(report.freed, 0);
    assert_eq!(
        fs::read(&real).unwrap(),
        b"[Default Applications]\r\nimage/png=other.desktop\n\n[Added Associations]\ntext/plain=foo.desktop;bar.desktop\n"
    );
    assert_eq!(
        fs::metadata(&real).unwrap().permissions().mode() & 0o777,
        0o640
    );
    let backup = PathBuf::from(format!(
        "/home/user/.config/mimeapps.list.crumbs-bak-{}",
        report.batch
    ));
    assert_eq!(fs::read(env.sys_path(&backup)).unwrap(), MIME);
    let (hist, _) = read_history(&env).unwrap();
    assert_eq!(hist[0].action, HistoryAction::EditFile);
    assert_eq!(hist[0].backup.as_deref(), Some(backup.as_path()));
    // No temp file left behind.
    let names: Vec<String> = fs::read_dir(env.sys("/home/user/.config"))
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names.len(), 2, "{names:?}");

    // Nothing left to remove: ok, and no second backup.
    let plan = plan_of(vec![planned(&env, mime_edit())]);
    let again = execute(&env, &Config::default(), &plan);
    assert!(again.results[0].ok);
    assert!(
        !env.sys_path(Path::new(&format!(
            "/home/user/.config/mimeapps.list.crumbs-bak-{}",
            again.batch
        )))
        .exists()
    );
    let (hist, _) = read_history(&env).unwrap();
    assert_eq!(hist[1].backup, None);
}

#[test]
fn edit_file_refuses_symlink() {
    let t = FixtureTree::new();
    t.write("home/user/elsewhere.list", MIME);
    t.symlink("home/user/.config/mimeapps.list", "../elsewhere.list");
    let env = t.env();
    let plan = plan_of(vec![planned(&env, mime_edit())]);
    let report = execute(&env, &Config::default(), &plan);
    assert!(!report.results[0].ok);
    assert_eq!(
        report.results[0].error.as_deref(),
        Some("refusing: ~/.config/mimeapps.list is a symlink; edit it by hand")
    );
    assert!(
        fs::symlink_metadata(env.sys("/home/user/.config/mimeapps.list"))
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(
        fs::read(env.sys("/home/user/elsewhere.list")).unwrap(),
        MIME
    );
}

#[test]
fn delete_makes_your_read_only_folders_writable() {
    // A folder of yours without write permission used to leave a partial
    // delete; its permission is restored and the delete completes. (A
    // folder owned by someone else still ends in `partially deleted: ...`.)
    if rustix::process::getuid().is_root() {
        return; // root ignores directory permissions
    }
    let t = FixtureTree::new();
    t.write("home/user/.config/foo/a.txt", b"a");
    t.write("home/user/.config/foo/locked/inner", b"i");
    let locked = t.dir.path().join("home/user/.config/foo/locked");
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o500)).unwrap();
    let env = t.env();
    let plan = plan_of(vec![planned(&env, delete("/home/user/.config/foo"))]);
    let report = execute(&env, &Config::default(), &plan);
    let r = &report.results[0];
    assert!(r.ok, "{r:?}");
    assert!(!env.sys("/home/user/.config/foo").exists());
}

/// `--permanent` never descends into another filesystem: a tree holding a
/// mount point is refused untouched. Mounts can't be made in tests, so
/// the device lookup is faked for one subdirectory.
#[test]
fn delete_refuses_tree_with_another_filesystem() {
    use std::os::unix::fs::MetadataExt;
    let t = FixtureTree::new();
    t.write("home/user/.config/foo/a", b"a");
    t.write("home/user/.config/foo/mnt/inner", b"on another fs");
    let env = t.env();
    let mnt = env.sys("/home/user/.config/foo/mnt");
    // A same-device bind mount: only the mount ID differs.
    let fake_mount = |p: &Path, _: &fs::Metadata| Some(if p.starts_with(&mnt) { 2 } else { 1 });
    let err = delete_with(&env, Path::new("/home/user/.config/foo"), &fake_mount).unwrap_err();
    assert_eq!(err, "refusing: ~/.config/foo contains another filesystem");
    assert!(env.sys("/home/user/.config/foo/a").exists());
    assert!(env.sys("/home/user/.config/foo/mnt/inner").exists());

    // Same device everywhere: deleted as before.
    let same = |_: &Path, m: &fs::Metadata| Some(m.dev());
    delete_with(&env, Path::new("/home/user/.config/foo"), &same).unwrap();
    assert!(!env.sys("/home/user/.config/foo").exists());
}

/// A target that is itself a mount root (a FUSE mount in `/tmp`, say) is
/// refused untouched: its mount differs from its parent's.
#[test]
fn delete_refuses_target_that_is_a_mount_root() {
    let t = FixtureTree::new();
    t.write("tmp/mnt/inner", b"on another fs");
    let env = t.env();
    let mnt = env.sys("/tmp/mnt");
    let fake_mount = |p: &Path, _: &fs::Metadata| Some(if p.starts_with(&mnt) { 2 } else { 1 });
    let err = delete_with(&env, Path::new("/tmp/mnt"), &fake_mount).unwrap_err();
    assert_eq!(err, "refusing: /tmp/mnt contains another filesystem");
    assert!(env.sys("/tmp/mnt/inner").exists());
}

#[test]
fn history_failure_is_reported() {
    let t = FixtureTree::new();
    t.write("home/user/.config/foo/a", b"a");
    // A file where the state dir should be makes the append fail.
    t.write("home/user/.local/state/crumbs", b"not a dir");
    let env = t.env();
    let plan = plan_of(vec![planned(&env, trash("/home/user/.config/foo"))]);
    let report = execute(&env, &Config::default(), &plan);
    let err = report.results[0].error.as_deref().unwrap();
    assert!(err.ends_with("; history not written"), "{err}");
}

/// Each action's history entry is its own line: one entry that can't be
/// written (a non-UTF-8 path can't be serialised) loses only that entry.
#[test]
fn history_entry_failure_is_per_action() {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;
    let t = FixtureTree::new();
    t.write("home/user/.config/a/f", b"a");
    t.write("home/user/.config/c/f", b"c");
    let env = t.env();
    let bad = Path::new("/home/user/.config").join(OsStr::from_bytes(b"b\xff"));
    fs::write(env.sys_path(&bad), b"b").unwrap();
    let plan = plan_of(vec![
        planned(&env, trash("/home/user/.config/a")),
        planned(&env, Action::Trash { path: bad.clone() }),
        planned(&env, trash("/home/user/.config/c")),
    ]);
    let report = execute(&env, &Config::default(), &plan);
    assert!(report.results.iter().all(|r| r.ok), "{report:?}");
    assert_eq!(report.results[0].error, None);
    assert_eq!(report.results[2].error, None);
    let err = report.results[1].error.as_deref().unwrap();
    assert!(err.ends_with("; history not written"), "{err}");
    let (hist, skipped) = read_history(&env).unwrap();
    assert_eq!(skipped, 0);
    let ids: Vec<&str> = hist.iter().map(|h| h.id.as_str()).collect();
    assert_eq!(
        ids,
        [
            report.results[0].history_id.as_str(),
            report.results[2].history_id.as_str()
        ]
    );
}

#[test]
fn history_records_packages() {
    let t = FixtureTree::new();
    let log = t.dir.path().join("calls.log");
    logging_program(&t, "pacman", &log, "", 0);
    logging_program(&t, "flatpak", &log, "", 0);
    let env = t.env();
    let plan = plan_of(vec![
        pkg_remove(&["foo", "libfoo"], &["1.0-1", "2.1-3"]),
        priv_cmd("flatpak", &["uninstall", "--system", "-y", "org.x.App"]),
    ]);
    let report = execute_full(
        &env,
        &Config::default(),
        &plan,
        &FakeRunner::new(),
        &FakePrivilegedRunner::default(),
        Some(&esc()),
        &no_post_check,
    );
    assert!(report.results.iter().all(|r| r.ok), "{report:?}");
    let (hist, _) = read_history(&env).unwrap();
    assert_eq!(hist[0].action, HistoryAction::PackageRemove);
    assert_eq!(hist[0].path, Path::new("/usr/bin/pacman"));
    assert_eq!(
        hist[0].packages.as_deref(),
        Some(
            &[
                ("foo".to_string(), "1.0-1".to_string()),
                ("libfoo".into(), "2.1-3".into())
            ][..]
        )
    );
    assert_eq!(
        hist[0].argv.as_deref(),
        Some(
            &[
                "-Rns".to_string(),
                "--noconfirm".into(),
                "foo".into(),
                "libfoo".into()
            ][..]
        )
    );
    assert_eq!(hist[0].size, 100);
    assert_eq!(hist[1].action, HistoryAction::PrivilegedCommand);
    assert_eq!(hist[1].path, Path::new("/usr/bin/flatpak"));
    assert_eq!(hist[1].packages, None);
    assert_eq!(
        hist[1].argv.as_deref(),
        Some(
            &[
                "uninstall".to_string(),
                "--system".into(),
                "-y".into(),
                "org.x.App".into()
            ][..]
        )
    );
    let text = fs::read_to_string(env.state_home.join("crumbs/history.jsonl")).unwrap();
    assert!(text.contains("\"action\":\"package-remove\""), "{text}");
    assert!(text.contains("\"packages\":[[\"foo\",\"1.0-1\"]"), "{text}");
    assert!(text.contains("\"action\":\"privileged-command\""), "{text}");
}
