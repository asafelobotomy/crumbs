use super::*;

#[test]
fn escalation_argv_per_kind() {
    let script = b"echo hi";
    let argv = |kind| {
        let e = Escalation {
            kind,
            program: PathBuf::from("/usr/bin/x"),
            requires_tty: false,
        };
        escalation_args(&e, script)
    };
    let os = |s: &str| OsString::from(s);
    assert_eq!(
        argv(EscalationKind::Run0),
        [os("--"), os("/bin/sh"), os("-c"), os("echo hi")]
    );
    assert_eq!(argv(EscalationKind::AlreadyRoot), [os("-c"), os("echo hi")]);
    assert_eq!(
        argv(EscalationKind::Pkexec),
        [os("/bin/sh"), os("-c"), os("echo hi")]
    );
    assert_eq!(
        argv(EscalationKind::Sudo),
        [os("--"), os("/bin/sh"), os("-c"), os("echo hi")]
    );
}

#[test]
fn terminal_behaviour_unchanged() {
    assert_eq!(
        stdio_modes(Interaction::Terminal),
        (StdioMode::Inherit, StdioMode::Inherit)
    );
    let r = SystemPrivilegedRunner::new(Path::new("/"));
    assert_eq!(r.interaction(), Interaction::Terminal);
    // A runner that does not capture returns empty stderr.
    let f = FakePrivilegedRunner::default();
    let run = f.run_captured(&esc(), b"echo out; echo err >&2").unwrap();
    assert_eq!(run.stdout, b"out\n");
    assert!(run.stderr.is_empty());
}

#[test]
fn captured_stderr_is_capped_to_its_tail() {
    let f = FakePrivilegedRunner::with_interaction(Interaction::NoTerminal);
    // 100000 'a' then a distinctive tail, all on stderr.
    let script = b"head -c 100000 /dev/zero | tr '\\0' a >&2; printf TAIL >&2";
    let run = f.run_captured(&esc(), script).unwrap();
    let prefix = "… (truncated)\n".as_bytes();
    assert_eq!(run.stderr.len(), prefix.len() + MAX_STDERR_BYTES);
    assert!(run.stderr.starts_with(prefix));
    assert!(run.stderr.ends_with(b"aTAIL"));
    // Under the cap it is untouched.
    assert_eq!(cap_stderr(b"small".to_vec()), b"small");
    let exact = vec![b'x'; MAX_STDERR_BYTES];
    assert_eq!(cap_stderr(exact.clone()), exact);
}

#[test]
fn no_terminal_runner_uses_null_stdin() {
    assert_eq!(
        stdio_modes(Interaction::NoTerminal),
        (StdioMode::Null, StdioMode::Piped)
    );
    let r = SystemPrivilegedRunner::with_interaction(Path::new("/"), Interaction::NoTerminal);
    assert_eq!(r.interaction(), Interaction::NoTerminal);
    // The fake records the interaction it ran with and captures stderr.
    let f = FakePrivilegedRunner::with_interaction(Interaction::NoTerminal);
    let run = f.run_captured(&esc(), b"echo out; echo err >&2").unwrap();
    assert_eq!(run.stdout, b"out\n");
    assert_eq!(run.stderr, b"err\n");
    assert_eq!(*f.interactions.lock().unwrap(), [Interaction::NoTerminal]);
}

#[test]
fn fake_runner_records_scripts() {
    let r = FakePrivilegedRunner::default();
    r.run(&esc(), b"echo one").unwrap();
    r.run(&esc(), b"echo two").unwrap();
    assert_eq!(
        *r.calls.lock().unwrap(),
        [b"echo one".to_vec(), b"echo two".to_vec()]
    );
}

/// Review Focus: no test may ever escalate. The escalation program here
/// is a harmless fixture that only logs, in case the guard is missing.
#[test]
fn system_runner_refuses_in_tests_and_fake_roots() {
    let t = FixtureTree::new();
    let log = t.dir.path().join("esc.log");
    let prog = t.write_exec(
        "usr/bin/run0",
        format!("#!/bin/sh\necho ran >> '{}'\n", log.display()).as_bytes(),
    );
    let esc = Escalation {
        kind: EscalationKind::Run0,
        program: prog,
        requires_tty: false,
    };
    for root in [Path::new("/"), t.dir.path()] {
        let err = SystemPrivilegedRunner::new(root)
            .run(&esc, b"exit 0\n")
            .unwrap_err();
        assert!(err.to_string().contains("refusing to escalate"), "{err}");
    }
    assert!(!log.exists(), "the escalation program ran");
    // Outside tests only a fake root is refused.
    assert_eq!(escalation_refused(Path::new("/"), false), None);
    assert!(escalation_refused(t.dir.path(), false).is_some());
    assert!(escalation_refused(Path::new("/"), true).is_some());
}

#[test]
fn run_until_root_kills_a_child_that_never_reaches_root() {
    let mut cmd = Command::new("/bin/sh");
    cmd.args(["-c", "echo waiting; sleep 30"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let started = std::time::Instant::now();
    let run = run_until_root(cmd, Some(Duration::from_millis(300))).unwrap();
    assert!(run.timed_out);
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "it did not wait for sleep"
    );
}

#[test]
fn run_until_root_has_no_limit_once_root_runs() {
    let mut cmd = Command::new("/bin/sh");
    cmd.args([
        "-c",
        "printf '@@crumbs root\\n'; sleep 1; printf '@@crumbs 0 0\\n'",
    ])
    .stdin(Stdio::null())
    .stdout(Stdio::piped())
    .stderr(Stdio::piped());
    let run = run_until_root(cmd, Some(Duration::from_millis(300))).unwrap();
    assert!(!run.timed_out);
    assert_eq!(run.stdout, b"@@crumbs root\n@@crumbs 0 0\n");
}

#[test]
fn run_until_root_returns_when_the_child_ends_without_root() {
    let mut cmd = Command::new("/bin/sh");
    cmd.args(["-c", "echo denied >&2; exit 1"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let run = run_until_root(cmd, Some(Duration::from_secs(30))).unwrap();
    assert!(!run.timed_out);
    assert_eq!(run.stderr, b"denied\n");
}

#[test]
fn the_script_says_when_root_runs_and_markers_ignore_it() {
    let script = build_script(&[]).unwrap();
    assert!(
        script
            .windows(ROOT_LINE.len() - 1)
            .any(|w| w == &ROOT_LINE[..ROOT_LINE.len() - 1])
    );
    let outcomes = parse_markers(b"@@crumbs root\n@@crumbs 0 0\n", 1);
    assert_eq!(outcomes[0].status, Some(0));
}
