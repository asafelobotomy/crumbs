use super::*;

#[test]
fn denied_batch_keeps_the_escalation_stderr_and_a_run_one_does_not() {
    let t = FixtureTree::new();
    let log = t.dir.path().join("calls.log");
    logging_program(&t, "flatpak", &log, "", 0);
    let env = t.env();
    let plan = plan_of(vec![priv_cmd(
        "flatpak",
        &["uninstall", "--system", "-y", "org.x.App"],
    )]);
    let run = |pr: &dyn PrivilegedRunner| {
        execute_full(
            &env,
            &Config::default(),
            &plan,
            &FakeRunner::new(),
            pr,
            Some(&esc()),
            &|_, _, _| Ok(Vec::new()),
        )
    };
    let report = run(&DeniedWithStderr);
    assert!(report.escalation_failed);
    assert_eq!(report.escalation_output, b"polkit: not authorized\n");
    // Not in the JSON.
    let json = serde_json::to_string(&report).unwrap();
    assert!(!json.contains("polkit"), "{json}");
    // Markers came back: nothing kept, whatever was on stderr.
    let report = run(&FakePrivilegedRunner::with_interaction(
        crate::privilege::Interaction::NoTerminal,
    ));
    assert!(!report.escalation_failed, "{report:?}");
    assert!(report.escalation_output.is_empty());
}

#[test]
fn no_escalation_tool_reports_and_flags() {
    let t = FixtureTree::new();
    let log = t.dir.path().join("calls.log");
    logging_program(&t, "pacman", &log, "", 0);
    t.write("home/user/.config/foo/a", b"a");
    let env = t.env();
    let plan = plan_of(vec![
        pkg_remove(&["foo"], &["1.0-1"]),
        planned(&env, trash("/home/user/.config/foo")),
    ]);
    let pr = FakePrivilegedRunner::default();
    let report = execute_full(
        &env,
        &Config::default(),
        &plan,
        &FakeRunner::new(),
        &pr,
        None,
        &no_post_check,
    );
    assert!(report.escalation_failed);
    assert!(pr.calls.lock().unwrap().is_empty());
    assert_eq!(
        report.results[0].error.as_deref(),
        Some("no way to get administrator rights (install systemd ≥ 256, polkit or sudo)")
    );
    // An independent user action still runs.
    assert!(report.results[1].ok, "{:?}", report.results[1]);
    assert!(!log.exists());

    // `execute_with` never escalates.
    let t = FixtureTree::new();
    logging_program(&t, "pacman", &log, "", 0);
    let env = t.env();
    let plan = plan_of(vec![pkg_remove(&["foo"], &["1.0-1"])]);
    let report = execute_with(&env, &Config::default(), &plan, &FakeRunner::new());
    assert!(report.escalation_failed);
    assert_eq!(
        report.results[0].error.as_deref(),
        Some("no way to get administrator rights (install systemd ≥ 256, polkit or sudo)")
    );
}

#[test]
fn no_markers_is_a_denial() {
    let t = FixtureTree::new();
    let log = t.dir.path().join("calls.log");
    logging_program(&t, "flatpak", &log, "", 0);
    let env = t.env();
    let plan = plan_of(vec![
        priv_cmd("flatpak", &["a"]),
        priv_cmd("flatpak", &["b"]),
    ]);
    let report = execute_full(
        &env,
        &Config::default(),
        &plan,
        &FakeRunner::new(),
        &CannedRunner(b"noise\n|not a marker\n".to_vec()),
        Some(&esc()),
        &no_post_check,
    );
    assert!(report.escalation_failed);
    assert!(!report.batch_stopped);
    for r in &report.results {
        assert_eq!(r.error.as_deref(), Some(ESCALATION_DENIED));
        assert!(r.rights_missing);
    }
}

#[test]
fn system_delete_verified_before_batch() {
    let t = FixtureTree::new();
    fixture_rm(&t);
    coredump_dir(&t);
    t.write("var/lib/systemd/coredump/core.a.1", b"a");
    t.write("var/lib/systemd/coredump/core.b.1", b"b");
    t.write("etc/precious", b"keep me");
    let env = t.env();
    let plan = plan_of(vec![
        core_delete(&env, "core.a.1"),
        core_delete(&env, "core.b.1"),
    ]);
    // core.b.1 is swapped for a symlink to a precious file after the scan.
    let b = env.sys("/var/lib/systemd/coredump/core.b.1");
    fs::remove_file(&b).unwrap();
    std::os::unix::fs::symlink(env.sys("/etc/precious"), &b).unwrap();
    let pr = FakePrivilegedRunner::default();
    let report = execute_full(
        &env,
        &Config::default(),
        &plan,
        &FakeRunner::new(),
        &pr,
        Some(&esc()),
        &no_post_check,
    );
    assert!(report.results[0].ok, "{:?}", report.results[0]);
    assert!(!report.results[1].ok);
    assert!(!report.escalation_failed);
    let calls = pr.calls.lock().unwrap().clone();
    assert_eq!(calls.len(), 1);
    assert_eq!(step_count(&calls[0]), 1, "the swapped file is left out");
    let script = String::from_utf8_lossy(&calls[0]);
    assert!(script.contains("core.a.1"));
    assert!(!script.contains("core.b.1"));
    assert!(script.contains("'-f' '--'"), "{script}");
    assert!(!env.sys("/var/lib/systemd/coredump/core.a.1").exists());
    assert!(fs::symlink_metadata(&b).unwrap().file_type().is_symlink());
    assert_eq!(fs::read(env.sys("/etc/precious")).unwrap(), b"keep me");
}

#[test]
fn system_tree_delete_runs_one_rm_rf_step() {
    let Some(ns) = own_mount_ns() else { return };
    let (_t, env) = system_tree_fixture();
    let plan = plan_of(vec![
        tree_delete(&env, "/opt/inky"),
        tree_delete(&env, "/opt/blot"),
    ]);
    let (report, calls) = run_tree(&env, &plan);
    for r in &report.results {
        assert!(r.ok, "{r:?}");
    }
    assert_eq!(calls.len(), 1, "one escalation for the whole run");
    assert_eq!(step_count(&calls[0]), 2);
    let script = String::from_utf8_lossy(&calls[0]);
    let guard = guard_script(&env);
    // The guard is one quoted word of the step, so its quotes are escaped.
    let quoted = String::from_utf8_lossy(&crate::privilege::batch::sh_quote(&guard)).into_owned();
    assert!(script.contains(&quoted), "{script}");
    assert!(script.contains("'crumbs-rm'"), "{script}");
    assert!(
        script.contains(&format!("'{}'", ns.to_string_lossy())),
        "{script}"
    );
    assert!(
        script.contains("'inky'") && script.contains("'blot'"),
        "{script}"
    );
    assert!(!env.sys("/opt/inky").exists());
    assert!(!env.sys("/opt/blot").exists());
    assert_eq!(report.freed, plan.total_size);
}

#[test]
fn system_tree_guard_text() {
    let env = FixtureTree::new().env();
    let lock = env.sys("/var/lib/pacman/db.lck").display().to_string();
    let readlink = env.sys("/usr/bin/readlink");
    let rm = env.sys("/usr/bin/rm");
    // Spelled out once here; every other test uses the builder.
    let want = format!(
        "{{ [ -e '{lock}' ] || [ -L '{lock}' ]; }} && {{ echo \"pacman is busy (database lock \
         present); not deleting\" >&2; exit 97; }}; test \"$('{}' /proc/self/ns/mnt)\" = \"$1\" || {{ echo \"mount \
         namespace differs; not deleting\" >&2; exit 97; }}; exec '{}' -rf --one-file-system \
         -- \"$2\"",
        readlink.display(),
        rm.display()
    );
    assert_eq!(guard_script(&env), std::ffi::OsString::from(want));
}

#[test]
fn system_tree_step_args_arrive_byte_exact() {
    let Some(ns) = own_mount_ns() else { return };
    let (t, env) = system_tree_fixture();
    // An `sh` that logs each argument, NUL-terminated, then runs the real one.
    let log = t.dir.path().join("sh.log");
    let sh = format!(
        "#!/bin/sh\nprintf '%s\\0' \"$@\" >> '{}'\nexec /bin/sh \"$@\"\n",
        log.display()
    );
    t.write_exec("usr/bin/sh", sh.as_bytes());
    let plan = plan_of(vec![tree_delete(&env, "/opt/inky")]);
    let (report, _) = run_tree(&env, &plan);
    assert!(report.results[0].ok, "{:?}", report.results[0]);
    let mut expected: Vec<u8> = Vec::new();
    for arg in [
        std::ffi::OsString::from("-c"),
        guard_script(&env),
        "crumbs-rm".into(),
        ns,
        env.sys("/opt/inky").into_os_string(),
    ] {
        expected.extend_from_slice(arg.as_encoded_bytes());
        expected.push(0);
    }
    assert_eq!(fs::read(&log).unwrap(), expected);
}

#[test]
fn system_tree_delete_refused_when_namespace_differs() {
    let (_t, mut env) = system_tree_fixture();
    env.set_mount_ns(Some("mnt:[1]".into()));
    let plan = plan_of(vec![tree_delete(&env, "/opt/inky")]);
    let (report, calls) = run_tree(&env, &plan);
    let r = &report.results[0];
    assert!(!r.ok);
    assert_eq!(
        r.error.as_deref(),
        Some("mount namespace differs; not deleting"),
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
fn system_tree_delete_refused_when_namespace_unreadable() {
    let (_t, mut env) = system_tree_fixture();
    env.set_mount_ns(None);
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
fn system_tree_delete_refused_when_mount_below() {
    let (t, env) = system_tree_fixture();
    let info = [
        mountinfo_line(Path::new("/")),
        mountinfo_line(&t.dir.path().join("opt/inky/data")),
    ]
    .concat();
    t.write("proc/self/mountinfo", info.as_bytes());
    let plan = plan_of(vec![
        tree_delete(&env, "/opt/inky"),
        tree_delete(&env, "/opt/blot"),
    ]);
    let (report, calls) = run_tree(&env, &plan);
    assert_eq!(
        report.results[0].error.as_deref(),
        Some("a mount point is below it")
    );
    assert!(report.results[1].ok, "{:?}", report.results[1]);
    assert_eq!(step_count(&calls[0]), 1);
    assert!(!String::from_utf8_lossy(&calls[0]).contains("inky"));
    assert!(env.sys("/opt/inky/a").exists());
    assert!(!env.sys("/opt/blot").exists());
}

#[test]
fn system_tree_delete_refused_when_mountinfo_unreadable() {
    let (t, env) = system_tree_fixture();
    t.write("proc/self/mountinfo", b"");
    let plan = plan_of(vec![tree_delete(&env, "/opt/inky")]);
    let (report, calls) = run_tree(&env, &plan);
    assert_eq!(
        report.results[0].error.as_deref(),
        Some("the mount table could not be read")
    );
    assert!(calls.is_empty());
    assert!(env.sys("/opt/inky/a").exists());
}
