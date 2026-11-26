use super::*;

#[test]
fn system_tree_delete_refused_when_owned_since_scan() {
    let (t, env) = system_tree_fixture();
    let plan = plan_of(vec![
        tree_delete(&env, "/opt/inky"),
        tree_delete(&env, "/opt/blot"),
    ]);
    t.pacman_pkg("inky", "2-1", Some(1), &["opt/inky/new"]);
    let (report, _) = run_tree(&env, &plan);
    assert_eq!(
        report.results[0].error.as_deref(),
        Some("a package owns files in it now")
    );
    assert!(report.results[1].ok, "{:?}", report.results[1]);
    assert!(env.sys("/opt/inky/a").exists());
}

#[test]
fn system_tree_delete_refused_while_pacman_locked() {
    let (t, env) = system_tree_fixture();
    t.write("var/lib/pacman/db.lck", b"");
    let plan = plan_of(vec![tree_delete(&env, "/opt/inky")]);
    let (report, calls) = run_tree(&env, &plan);
    assert_eq!(
        report.results[0].error.as_deref(),
        Some("pacman is busy (database lock present)")
    );
    assert!(calls.is_empty());
    assert!(env.sys("/opt/inky/a").exists());
}

#[test]
fn system_tree_delete_refused_while_a_package_manager_runs() {
    for name in ["dpkg", "apt-get", "dnf5", "rpm", "unattended-upgr"] {
        let (t, env) = system_tree_fixture();
        t.write("proc/4242/comm", format!("{name}\n").as_bytes());
        let plan = plan_of(vec![tree_delete(&env, "/opt/inky")]);
        let (report, calls) = run_tree(&env, &plan);
        assert_eq!(
            report.results[0].error.as_deref(),
            Some(TREE_PM_RUNNING),
            "{name}"
        );
        assert!(calls.is_empty());
        assert!(env.sys("/opt/inky/a").exists());
    }
    // An unrelated process does not block.
    let (t, env) = system_tree_fixture();
    t.write("proc/4242/comm", b"bash\n");
    let plan = plan_of(vec![tree_delete(&env, "/opt/inky")]);
    let (report, _) = run_tree(&env, &plan);
    assert_ne!(report.results[0].error.as_deref(), Some(TREE_PM_RUNNING));
}

#[test]
fn system_tree_delete_refused_without_package_database() {
    let (t, env) = system_tree_fixture();
    fs::remove_dir_all(t.dir.path().join("var/lib/pacman/local")).unwrap();
    let plan = plan_of(vec![tree_delete(&env, "/opt/inky")]);
    let (report, calls) = run_tree(&env, &plan);
    assert_eq!(
        report.results[0].error.as_deref(),
        Some("the package database could not be read")
    );
    assert!(calls.is_empty());
    assert!(env.sys("/opt/inky/a").exists());

    // An unreadable entry fails closed the same way.
    let (t, env) = system_tree_fixture();
    t.write("var/lib/pacman/local/bad-1-1/files/x", b"");
    let plan = plan_of(vec![tree_delete(&env, "/opt/inky")]);
    let (report, _) = run_tree(&env, &plan);
    assert_eq!(
        report.results[0].error.as_deref(),
        Some("the package database could not be read")
    );
    assert!(env.sys("/opt/inky/a").exists());
}

#[test]
fn system_tree_history_and_restore() {
    let (_t, env) = system_tree_fixture();
    let plan = plan_of(vec![tree_delete(&env, "/opt/inky")]);
    let (report, _) = run_tree(&env, &plan);
    assert!(report.results[0].ok, "{:?}", report.results[0]);
    let raw = fs::read_to_string(env.state_home.join("crumbs/history.jsonl")).unwrap();
    assert!(raw.contains("\"scope\":\"system-tree\""), "{raw}");
    let (hist, _) = read_history(&env).unwrap();
    assert_eq!(hist[0].scope, Some(Scope::SystemTree));
    let cfg = Config::default();
    let id = report.results[0].history_id.clone();
    let out = crate::restore::restore(&env, &cfg, &[id], false).unwrap();
    assert_eq!(out.len(), 1);
    assert!(!out[0].ok);
    assert_eq!(
        out[0].error.as_deref(),
        Some(crate::restore::MSG_SYSTEM_TREE)
    );
    let out =
        crate::restore::restore(&env, &cfg, std::slice::from_ref(&report.batch), false).unwrap();
    assert_eq!(out.len(), 1);
    assert!(out[0].ok, "{out:?}");
    assert_eq!(
        out[0].error.as_deref(),
        Some(crate::restore::NOTHING_RESTORABLE)
    );
}

#[test]
fn system_tree_step_refuses_when_lock_appears_in_the_batch() {
    let (t, env) = system_tree_fixture();
    // The lock appears after the user-side re-check: the step's `sh`
    // creates it just before running the guard.
    let lock = env.sys("/var/lib/pacman/db.lck");
    let sh = format!("#!/bin/sh\n: > '{}'\nexec /bin/sh \"$@\"\n", lock.display());
    t.write_exec("usr/bin/sh", sh.as_bytes());
    let plan = plan_of(vec![tree_delete(&env, "/opt/inky")]);
    let (report, calls) = run_tree(&env, &plan);
    let r = &report.results[0];
    assert!(!r.ok);
    assert_eq!(
        r.error.as_deref(),
        Some("pacman is busy (database lock present); not deleting"),
        "{r:?}"
    );
    assert!(env.sys("/opt/inky/sub/b").exists());
    // The step itself exited 97.
    let out = std::process::Command::new("/bin/sh")
        .arg("-c")
        .arg(std::ffi::OsStr::from_bytes(&calls[0]))
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap();
    let markers = crate::privilege::batch::parse_markers(&out.stdout, 1);
    assert_eq!(markers[0].status, Some(97));
    assert!(env.sys("/opt/inky/sub/b").exists());
}

#[test]
fn system_tree_delete_refused_when_tmpfiles_declares_it_now() {
    let (t, env) = system_tree_fixture();
    let plan = plan_of(vec![
        tree_delete(&env, "/opt/inky"),
        tree_delete(&env, "/opt/blot"),
    ]);
    t.write("usr/lib/tmpfiles.d/inky.conf", b"d /opt/inky/run 0755\n");
    let (report, calls) = run_tree(&env, &plan);
    assert_eq!(
        report.results[0].error.as_deref(),
        Some("a package owns files in it now")
    );
    assert!(report.results[1].ok, "{:?}", report.results[1]);
    assert_eq!(step_count(&calls[0]), 1);
    assert!(env.sys("/opt/inky/a").exists());
    assert!(!env.sys("/opt/blot").exists());

    // Unreadable `tmpfiles.d` fails closed.
    let (t, env) = system_tree_fixture();
    t.write("etc/tmpfiles.d", b"not a directory");
    let plan = plan_of(vec![tree_delete(&env, "/opt/inky")]);
    let (report, calls) = run_tree(&env, &plan);
    assert_eq!(
        report.results[0].error.as_deref(),
        Some("the package database could not be read")
    );
    assert!(calls.is_empty());
    assert!(env.sys("/opt/inky/a").exists());
}

#[test]
fn system_tree_rechecks_read_pacman_once_per_batch() {
    let (_t, env) = system_tree_fixture();
    let plan = plan_of(vec![
        tree_delete(&env, "/opt/inky"),
        tree_delete(&env, "/opt/blot"),
    ]);
    super::super::PACMAN_LOADS.with(|n| n.set(0));
    let (report, _) = run_tree(&env, &plan);
    assert!(report.results.iter().all(|r| r.ok), "{report:?}");
    assert_eq!(super::super::PACMAN_LOADS.with(std::cell::Cell::get), 1);
}

#[test]
fn system_tree_delete_refused_without_trusted_readlink() {
    let (t, env) = system_tree_fixture();
    fs::remove_file(t.dir.path().join("usr/bin/readlink")).unwrap();
    let plan = plan_of(vec![tree_delete(&env, "/opt/inky")]);
    let (report, calls) = run_tree(&env, &plan);
    assert_eq!(
        report.results[0].error.as_deref(),
        Some("the mount namespace could not be read")
    );
    assert!(calls.is_empty());
    assert!(env.sys("/opt/inky/a").exists());
}

#[test]
fn system_tree_step_refuses_dangling_symlink_lock() {
    let (t, env) = system_tree_fixture();
    // A dangling symlink appears at the lock inside the batch.
    let lock = env.sys("/var/lib/pacman/db.lck");
    let sh = format!(
        "#!/bin/sh\nln -s /nonexistent-crumbs-target '{}'\nexec /bin/sh \"$@\"\n",
        lock.display()
    );
    t.write_exec("usr/bin/sh", sh.as_bytes());
    let plan = plan_of(vec![tree_delete(&env, "/opt/inky")]);
    let (report, _) = run_tree(&env, &plan);
    let r = &report.results[0];
    assert!(!r.ok);
    assert_eq!(
        r.error.as_deref(),
        Some("pacman is busy (database lock present); not deleting"),
        "{r:?}"
    );
    assert!(
        fs::symlink_metadata(&lock)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert!(!lock.exists(), "dangling");
    assert!(env.sys("/opt/inky/sub/b").exists());
}

#[test]
fn empty_batch_never_escalates() {
    let t = FixtureTree::new();
    fixture_rm(&t);
    coredump_dir(&t);
    t.write("var/lib/systemd/coredump/core.a.1", b"a");
    let env = t.env();
    let plan = plan_of(vec![
        core_delete(&env, "core.a.1"),
        // Not in a trusted directory: not installed.
        priv_cmd("no-such-tool", &["x"]),
    ]);
    fs::remove_file(env.sys("/var/lib/systemd/coredump/core.a.1")).unwrap();
    let pr = DeniedRunner::default();
    let report = execute_full(
        &env,
        &Config::default(),
        &plan,
        &FakeRunner::new(),
        &pr,
        Some(&esc()),
        &no_post_check,
    );
    assert_eq!(*pr.calls.lock().unwrap(), 0);
    assert!(!report.escalation_failed);
    assert!(!report.results[0].ok);
    assert_eq!(
        report.results[1].error.as_deref(),
        Some("no-such-tool is not installed")
    );
    // No escalation tool and nothing to escalate for: not a failure.
    let report = execute_full(
        &env,
        &Config::default(),
        &plan_of(vec![priv_cmd("no-such-tool", &[])]),
        &FakeRunner::new(),
        &pr,
        None,
        &no_post_check,
    );
    assert!(!report.escalation_failed);
}

#[test]
fn batch_error_fails_privileged_actions_without_escalation_failure() {
    let t = FixtureTree::new();
    let log = t.dir.path().join("calls.log");
    logging_program(&t, "flatpak", &log, "", 0);
    let env = t.env();
    let plan = plan_of(vec![
        priv_cmd("flatpak", &["bad\0arg"]),
        priv_cmd("flatpak", &["fine"]),
    ]);
    let pr = DeniedRunner::default();
    let report = execute_full(
        &env,
        &Config::default(),
        &plan,
        &FakeRunner::new(),
        &pr,
        Some(&esc()),
        &no_post_check,
    );
    assert_eq!(*pr.calls.lock().unwrap(), 0);
    assert!(!report.escalation_failed);
    for r in &report.results {
        assert!(!r.ok);
        assert!(r.error.as_deref().unwrap().contains("NUL byte"), "{r:?}");
    }
}
