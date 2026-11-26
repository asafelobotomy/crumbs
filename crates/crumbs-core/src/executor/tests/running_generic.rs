use super::*;

#[test]
fn running_process_detected_by_exe() {
    let t = FixtureTree::new();
    let env = t.env();
    t.symlink("proc/1234/exe", "/usr/bin/foo-bin");
    t.symlink("proc/99/exe", "/usr/bin/other");
    t.symlink("proc/self/exe", "/usr/bin/foo-bin");
    let loc = t.dir.path().join("home/user/Apps/Foo.AppImage");
    t.symlink("proc/555/exe", loc.to_str().unwrap());
    t.write("proc/777/status", b"no exe here");
    assert_eq!(running_processes(&env, &appimage_app()), [555, 1234]);
}

#[test]
fn running_process_detected_by_appimage_env() {
    let t = FixtureTree::new();
    let env = t.env();
    t.write(
        "proc/42/environ",
        b"HOME=/home/user\0APPIMAGE=/home/user/Apps/Foo.AppImage\0X=1\0",
    );
    t.write(
        "proc/43/environ",
        b"APPIMAGE=/home/user/Apps/Foo.AppImage.old\0",
    );
    t.symlink("proc/43/exe", "/tmp/.mount_x/usr/bin/other");
    assert_eq!(running_processes(&env, &appimage_app()), [42]);
}

#[test]
fn running_detects_deleted_exe_and_comm() {
    let t = FixtureTree::new();
    let env = t.env();
    // The binary was replaced by an update: the kernel appends " (deleted)".
    t.symlink("proc/10/exe", "/usr/bin/foo-bin (deleted)");
    // Only the process name and argv0 say who it is.
    t.write("proc/11/comm", b"foo-bin\n");
    t.write("proc/12/cmdline", b"/opt/foo/foo-bin\0--flag\0");
    // The kernel cuts comm at 15 bytes.
    t.write("proc/13/comm", b"very-long-name-\n");
    t.write("proc/14/comm", b"unrelated\n");
    t.write("proc/14/cmdline", b"/usr/bin/unrelated\0");
    let mut app = appimage_app();
    app.exec_names.push("very-long-name-app".into());
    assert_eq!(running_processes(&env, &app), [10, 11, 12, 13]);
}

#[test]
fn running_detects_script_launched_electron_app() {
    let t = FixtureTree::new();
    let env = t.env();
    let mut app = App::new(SourceKind::Unmanaged, "claude");
    app.exec_names = vec!["claude".into()];
    app.program = Some("/usr/lib/claude/claude".into());
    app.program_link = Some("/usr/bin/claude".into());
    // A launcher script run by the shell: argv1 is the script.
    t.symlink("proc/30/exe", "/usr/bin/bash");
    t.write("proc/30/cmdline", b"/bin/sh\0/usr/bin/claude\0--flag\0");
    // An interpreter running the canonical program.
    t.symlink("proc/31/exe", "/usr/bin/node");
    t.write("proc/31/cmdline", b"node\0/usr/lib/claude/claude\0");
    // A generic interpreter running something else, or the name only as a
    // later argument, never matches.
    t.symlink("proc/32/exe", "/usr/bin/node");
    t.write("proc/32/cmdline", b"node\0/usr/lib/other/cli.js\0");
    t.write("proc/33/cmdline", b"node\0x.js\0/usr/bin/claude\0");
    t.write("proc/34/cmdline", b"/bin/sh\0/home/user/claude\0");
    assert_eq!(running_processes(&env, &app), [30, 31]);
}

#[test]
fn running_processes_without_proc_is_empty() {
    let t = FixtureTree::new();
    let env = t.env();
    assert!(running_processes(&env, &appimage_app()).is_empty());
}

#[test]
fn generic_basenames_never_match() {
    let t = FixtureTree::new();
    let env = t.env();
    let mut app = appimage_app();
    app.exec_names = ["sh", "bash", "env", "python", "python3", "node"]
        .into_iter()
        .chain(["electron", "java", "flatpak", "bwrap", "/usr/bin/bash"])
        .chain(["dash", "zsh", "fish", "perl", "ruby", "python3.12"])
        .map(String::from)
        .collect();
    for (pid, name) in [(1, "sh"), (2, "bash"), (3, "python3"), (4, "flatpak")] {
        t.symlink(&format!("proc/{pid}/exe"), &format!("/usr/bin/{name}"));
    }
    t.write("proc/5/comm", b"node\n");
    t.write("proc/6/cmdline", b"/usr/bin/java\0-jar\0x.jar\0");
    t.write("proc/7/cmdline", b"bwrap\0--x\0");
    for (pid, name) in [
        (20, "dash"),
        (21, "zsh"),
        (22, "fish"),
        (23, "perl"),
        (24, "ruby"),
        (25, "python3.12"),
    ] {
        t.symlink(&format!("proc/{pid}/exe"), &format!("/usr/bin/{name}"));
    }
    assert!(running_processes(&env, &app).is_empty());
    // The exact location still matches.
    let loc = t.dir.path().join("home/user/Apps/Foo.AppImage");
    t.symlink("proc/8/exe", loc.to_str().unwrap());
    assert_eq!(running_processes(&env, &app), [8]);
}

#[test]
fn leftovers_skipped_when_appimage_file_fails() {
    if rustix::process::getuid().is_root() {
        return; // root ignores directory permissions
    }
    let t = FixtureTree::new();
    t.appimage("home/user/Apps/Foo.AppImage");
    t.write("home/user/.config/foo/a", b"a");
    t.write("home/user/.cache/foo/b", b"b");
    // A read-only Trash/info makes trashing anything fail.
    t.mkdir("home/user/.local/share/Trash/files");
    let info = t.mkdir("home/user/.local/share/Trash/info");
    let env = t.env();
    let mut actions = vec![
        planned(&env, trash("/home/user/Apps/Foo.AppImage")),
        planned(&env, trash("/home/user/.config/foo")),
        planned(&env, trash("/home/user/.cache/foo")),
    ];
    for a in &mut actions[1..] {
        a.depends_on = Some(0);
    }
    let plan = plan_of(actions);
    fs::set_permissions(&info, fs::Permissions::from_mode(0o500)).unwrap();
    let report = execute(&env, &Config::default(), &plan);
    fs::set_permissions(&info, fs::Permissions::from_mode(0o700)).unwrap();

    assert!(!report.results[0].ok);
    for r in &report.results[1..] {
        assert!(!r.ok);
        assert_eq!(
            r.error.as_deref(),
            Some("skipped: ~/Apps/Foo.AppImage was not removed")
        );
    }
    assert_eq!(report.freed, 0);
    assert!(env.sys("/home/user/.config/foo/a").exists());
    assert!(env.sys("/home/user/.cache/foo/b").exists());
    let (hist, _) = read_history(&env).unwrap();
    assert_eq!(hist.len(), 3);
    for (h, r) in hist.iter().zip(&report.results).skip(1) {
        assert_eq!(h.id, r.history_id);
        assert!(!h.ok);
        assert_eq!(h.action, HistoryAction::Trash);
        assert_eq!(h.error, r.error);
        assert!(h.trashed.is_none());
    }
}

#[test]
fn dependents_run_when_dependency_succeeds() {
    let t = FixtureTree::new();
    t.write("home/user/.config/foo/a", b"a");
    t.write("home/user/.cache/foo/b", b"b");
    let env = t.env();
    let mut second = planned(&env, trash("/home/user/.cache/foo"));
    second.depends_on = Some(0);
    let plan = plan_of(vec![planned(&env, trash("/home/user/.config/foo")), second]);
    let report = execute(&env, &Config::default(), &plan);
    assert!(report.results.iter().all(|r| r.ok), "{report:?}");
}

#[test]
fn invalid_dependencies_fail_closed_and_skips_are_transitive() {
    if rustix::process::getuid().is_root() {
        return; // root ignores directory permissions
    }
    let t = FixtureTree::new();
    t.write("home/user/.config/a/x", b"a");
    t.write("home/user/.config/b/x", b"b");
    t.write("home/user/.config/c/x", b"c");
    t.write("home/user/.config/d/x", b"d");
    t.write("home/user/.config/e/x", b"e");
    t.mkdir("home/user/.local/share/Trash/files");
    let info = t.mkdir("home/user/.local/share/Trash/info");
    let env = t.env();
    // 0 fails; 1 depends on 0; 2 on 1 (transitive); 3 on itself; 4 forward.
    let mut actions: Vec<PlannedAction> = ["a", "b", "c", "d", "e"]
        .iter()
        .map(|n| planned(&env, trash(&format!("/home/user/.config/{n}"))))
        .collect();
    actions[1].depends_on = Some(0);
    actions[2].depends_on = Some(1);
    actions[3].depends_on = Some(3);
    actions[4].depends_on = Some(9);
    let plan = plan_of(actions);
    fs::set_permissions(&info, fs::Permissions::from_mode(0o500)).unwrap();
    let report = execute(&env, &Config::default(), &plan);
    fs::set_permissions(&info, fs::Permissions::from_mode(0o700)).unwrap();
    let errs: Vec<&str> = report
        .results
        .iter()
        .map(|r| r.error.as_deref().unwrap())
        .collect();
    assert!(report.results.iter().all(|r| !r.ok));
    assert_eq!(errs[1], "skipped: ~/.config/a was not removed");
    assert_eq!(errs[2], "skipped: ~/.config/b was not removed");
    assert_eq!(errs[3], "skipped: ~/.config/d was not removed");
    assert_eq!(errs[4], "skipped: action 9 was not removed");
    for n in ["a", "b", "c", "d", "e"] {
        assert!(env.sys(&format!("/home/user/.config/{n}/x")).exists());
    }
}

#[test]
fn user_command_runs_trusted_program() {
    let t = FixtureTree::new();
    let log = t.dir.path().join("args.log");
    let script = format!("#!/bin/sh\necho \"$@\" > {}\n", log.display());
    t.write_exec("usr/bin/flatpak", script.as_bytes());
    let env = t.env();
    let plan = command_plan("flatpak", &["uninstall", "--user", "--unused"]);
    let report = execute(&env, &Config::default(), &plan);
    assert!(report.results[0].ok, "{:?}", report.results[0]);
    assert_eq!(
        fs::read_to_string(&log).unwrap(),
        "uninstall --user --unused\n"
    );
    assert_eq!(report.freed, 0);
    let (entries, _) = read_history(&env).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].action, HistoryAction::Command);
    assert_eq!(entries[0].path, Path::new("/usr/bin/flatpak"));
    assert_eq!(
        entries[0].argv.as_deref(),
        Some(&["uninstall".to_string(), "--user".into(), "--unused".into()][..])
    );
    assert!(entries[0].ok);
    assert_eq!(entries[0].size, 0);
    let line = fs::read_to_string(env.state_home.join("crumbs/history.jsonl")).unwrap();
    assert!(line.contains("\"action\":\"command\""), "{line}");
}

#[test]
fn user_command_missing_program() {
    let t = FixtureTree::new();
    // Only $HOME/.local/bin has it: not a trusted directory.
    t.write_exec("home/user/.local/bin/flatpak", b"#!/bin/sh\n");
    let env = t.env();
    let runner = FakeRunner::new();
    let plan = command_plan("flatpak", &["uninstall"]);
    let report = execute_with(&env, &Config::default(), &plan, &runner);
    assert!(!report.results[0].ok);
    assert_eq!(
        report.results[0].error.as_deref(),
        Some("flatpak is not installed")
    );
    assert!(runner.calls().is_empty());
    let (entries, _) = read_history(&env).unwrap();
    assert_eq!(entries[0].action, HistoryAction::Command);
    assert!(!entries[0].ok);
}

#[test]
fn user_command_failure_reports_last_stderr_line() {
    let t = FixtureTree::new();
    t.write_exec("usr/bin/flatpak", b"#!/bin/sh\n");
    let env = t.env();
    let out = |status, stderr: &str| CmdOutput {
        status,
        stdout: String::new(),
        stderr: stderr.into(),
        timed_out: false,
    };
    let runner = FakeRunner::new()
        .on("flatpak", &["a"], out(1, "first\nerror: boom\n\n"))
        .on("flatpak", &["b"], out(3, ""));
    let mut plan = command_plan("flatpak", &["a"]);
    plan.actions.extend(command_plan("flatpak", &["b"]).actions);
    let report = execute_with(&env, &Config::default(), &plan, &runner);
    assert_eq!(report.results[0].error.as_deref(), Some("error: boom"));
    assert_eq!(
        report.results[1].error.as_deref(),
        Some("exited with status 3")
    );
}

#[test]
fn user_command_timeout_is_a_failure() {
    let t = FixtureTree::new();
    t.write_exec("usr/bin/flatpak", b"#!/bin/sh\n");
    let env = t.env();
    let runner = FakeRunner::new().on(
        "flatpak",
        &["a"],
        CmdOutput {
            status: -1,
            stdout: String::new(),
            stderr: String::new(),
            timed_out: true,
        },
    );
    let report = execute_with(
        &env,
        &Config::default(),
        &command_plan("flatpak", &["a"]),
        &runner,
    );
    assert!(!report.results[0].ok);
    assert_eq!(
        report.results[0].error.as_deref(),
        Some("flatpak timed out after 30 min")
    );
}
