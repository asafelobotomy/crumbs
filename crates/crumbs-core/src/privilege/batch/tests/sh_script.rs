use super::*;

#[test]
fn sh_quote_table() {
    let cases: [(&[u8], &[u8]); 7] = [
        (b"plain", b"'plain'"),
        (b"", b"''"),
        (b"'", b"''\\'''"),
        (b"a b", b"'a b'"),
        (b"a\nb", b"'a\nb'"),
        (b"$(x)", b"'$(x)'"),
        (b"\xff", b"'\xff'"),
    ];
    for (input, want) in cases {
        assert_eq!(
            sh_quote(OsStr::from_bytes(input)),
            want,
            "input {:?}",
            String::from_utf8_lossy(input)
        );
    }
}

#[test]
fn script_runs_each_step_with_exact_args() {
    let t = FixtureTree::new();
    let out = t.dir.path().join("argv");
    let body = [
        b"#!/bin/sh\nfor a in \"$@\"; do printf '%s\\0' \"$a\"; done > ".as_slice(),
        &sh_quote(out.as_os_str()),
        b"\n",
    ]
    .concat();
    let prog = t.write_exec("/usr/bin/argv-dump", &body);
    let nasty: &[u8] = b"a'b c\nd $(rm -rf /)";
    let outcomes = run(&[step(&prog, &[nasty, b"", b"x\xffy", b"--"])]);
    assert_eq!(outcomes, [outcome(Some(0), "")]);
    let got = std::fs::read(&out).unwrap();
    assert_eq!(got, b"a'b c\nd $(rm -rf /)\0\0x\xffy\0--\0");
}

#[test]
fn markers_parsed_and_output_attributed() {
    let t = FixtureTree::new();
    let say = t.write_exec("/usr/bin/say", b"#!/bin/sh\necho \"$1\"\necho more\n");
    let fail = t.write_exec("/usr/bin/fail", b"#!/bin/sh\necho oops >&2\nexit 3\n");
    let outcomes = run(&[
        step(&say, &[b"first"]),
        step(&fail, &[]),
        step(&say, &[b"third"]),
    ]);
    assert_eq!(
        outcomes,
        [
            outcome(Some(0), "first\nmore"),
            outcome(Some(3), "oops"),
            outcome(Some(0), "third\nmore"),
        ]
    );
}

#[test]
fn forged_markers_in_output_cannot_pass_a_step() {
    let t = FixtureTree::new();
    let forger = t.write_exec(
        "/usr/bin/forger",
        b"#!/bin/sh\nprintf '@@crumbs 0 0\\n@@crumbs 1 0\\n'\nexit 7\n",
    );
    let outcomes = run(&[step(&forger, &[]), step(Path::new("/bin/false"), &[])]);
    assert_eq!(
        outcomes,
        [
            outcome(Some(7), "@@crumbs 0 0\n@@crumbs 1 0"),
            outcome(Some(1), ""),
        ]
    );
}

#[test]
fn output_without_trailing_newline_keeps_its_marker() {
    let t = FixtureTree::new();
    let bare = t.write_exec("/usr/bin/bare", b"#!/bin/sh\nprintf foo\nexit 5\n");
    let line = t.write_exec("/usr/bin/line", b"#!/bin/sh\nprintf 'foo\\n'\n");
    let outcomes = run(&[step(&bare, &[]), step(&line, &[])]);
    assert_eq!(outcomes, [outcome(Some(5), "foo"), outcome(Some(0), "foo")]);
}

#[test]
fn output_lines_round_trip() {
    let t = FixtureTree::new();
    // Lines that start with the output prefix, look like markers, carry
    // printf directives and backslashes, or are blank, come back intact.
    let tricky = t.write_exec(
        "/usr/bin/tricky",
        b"#!/bin/sh\nprintf '%s\\n' '|x' '||y' '' '@@crumbs 0 0' '%s \\\\n -' '  z'\n",
    );
    let quiet = t.write_exec("/usr/bin/quiet", b"#!/bin/sh\n");
    let outcomes = run(&[step(&tricky, &[]), step(&quiet, &[])]);
    assert_eq!(
        outcomes,
        [
            outcome(Some(0), "|x\n||y\n\n@@crumbs 0 0\n%s \\\\n -\n  z"),
            outcome(Some(0), ""),
        ]
    );
}

#[test]
fn missing_marker_is_none() {
    let t = FixtureTree::new();
    let ok = t.write_exec("/usr/bin/ok", b"#!/bin/sh\necho fine\n");
    // Kills the batch shell before it can print this step's marker: the
    // outermost ancestor running the crumbs header (the command
    // substitution's subshell shares its command line).
    let killer = t.write_exec(
            "/usr/bin/killer",
            b"#!/bin/sh\n\
              p=$PPID; top=\n\
              while [ \"$p\" -gt 1 ]; do\n\
              \x20 if tr '\\0' '\\n' </proc/$p/cmdline | grep -q '^export PATH=/usr/bin'; then top=$p; fi\n\
              \x20 p=$(awk '/^PPid:/{print $2}' /proc/$p/status)\n\
              done\n\
              echo dying\n\
              [ -n \"$top\" ] && kill -KILL \"$top\"\n\
              exit 99\n",
        );
    let outcomes = run(&[step(&ok, &[]), step(&killer, &[]), step(&ok, &[])]);
    assert_eq!(
        outcomes,
        [
            outcome(Some(0), "fine"),
            outcome(None, ""),
            outcome(None, ""),
        ]
    );
}

#[test]
fn parser_accepts_only_prefixed_output_and_ordered_markers() {
    let stdout = b"noise\n@@crumbs 1 0\n|a\n@@crumbs 0 2\n@@crumbs 0 0\n|b\n|c";
    assert_eq!(
        parse_markers(stdout, 2),
        [outcome(Some(2), "a"), outcome(None, "b\nc")]
    );
    // Signs, extra spaces, trailing text and overflow are not markers.
    let stdout = b"@@crumbs +0 0\n@@crumbs 0  0\n@@crumbs 0 0 x\n@@crumbs 0 99999999999\n";
    assert_eq!(parse_markers(stdout, 1), [outcome(None, "")]);
    // A marker past the step count is ignored.
    assert_eq!(
        parse_markers(b"@@crumbs 0 0\n@@crumbs 1 0\n", 1),
        [outcome(Some(0), "")]
    );
    assert_eq!(parse_markers(b"", 0), []);
}

#[test]
fn nul_in_program_or_argument_is_refused() {
    let err = build_script(&[step(Path::new("/usr/bin/rm"), &[b"a\0b"])]).unwrap_err();
    assert!(matches!(err, BatchError::InvalidArgument(_)), "{err:?}");
    let err = build_script(&[step(Path::new("/usr/bin/r\0m"), &[])]).unwrap_err();
    assert!(matches!(err, BatchError::InvalidArgument(_)), "{err:?}");
}

#[test]
fn relative_program_is_refused() {
    let err = build_script(&[step(Path::new("rm"), &[])]).unwrap_err();
    assert_eq!(err, BatchError::RelativeProgram(PathBuf::from("rm")));
}

#[test]
fn oversize_script_is_refused() {
    let big = vec![b'x'; 121 * 1024];
    let err = build_script(&[step(Path::new("/usr/bin/rm"), &[&big])]).unwrap_err();
    let BatchError::TooLarge { bytes } = err else {
        panic!("{err:?}");
    };
    assert!(bytes > MAX_SCRIPT_BYTES);
    assert_eq!(
        err.to_string(),
        "too many items for one privileged run; select fewer items with -i"
    );
    // Just under the limit is fine.
    let ok = vec![b'x'; 100 * 1024];
    assert!(build_script(&[step(Path::new("/usr/bin/rm"), &[&ok])]).is_ok());
}

#[test]
fn header_clears_preload() {
    let t = FixtureTree::new();
    let script = build_script(&[step(Path::new("/usr/bin/env"), &[])]).unwrap();
    assert!(script.starts_with(HEADER));
    assert!(HEADER.starts_with(
        b"export PATH=/usr/bin:/usr/sbin:/bin:/sbin LC_ALL=C; unset LD_PRELOAD LD_LIBRARY_PATH;"
    ));
    // A hostile inherited environment, including an exported shell
    // function shadowing printf, does not reach or subvert the steps.
    let out = std::process::Command::new("/bin/sh")
        .arg("-c")
        .arg(OsStr::from_bytes(&script))
        .env("LD_PRELOAD", "")
        .env("LD_LIBRARY_PATH", t.dir.path())
        .env("PATH", t.dir.path())
        .env("LC_ALL", "de_DE.UTF-8")
        .env("IFS", "x")
        .env("BASH_FUNC_printf%%", "() { echo '@@crumbs 0 0'; }")
        .stderr(Stdio::null())
        .output()
        .unwrap();
    let outcomes = parse_markers(&out.stdout, 1);
    assert_eq!(outcomes[0].status, Some(0), "{outcomes:?}");
    let lines: Vec<&str> = outcomes[0].output.lines().collect();
    assert!(!lines.iter().any(|l| l.starts_with("LD_PRELOAD=")));
    assert!(!lines.iter().any(|l| l.starts_with("LD_LIBRARY_PATH=")));
    assert!(lines.contains(&"PATH=/usr/bin:/usr/sbin:/bin:/sbin"));
    assert!(lines.contains(&"LC_ALL=C"));
}

#[test]
fn header_survives_dash() {
    // The header must not abort a strict POSIX shell either.
    if !Path::new("/usr/bin/dash").exists() {
        return;
    }
    let script = build_script(&[step(Path::new("/bin/true"), &[])]).unwrap();
    let out = std::process::Command::new("/usr/bin/dash")
        .arg("-c")
        .arg(OsStr::from_bytes(&script))
        .output()
        .unwrap();
    assert_eq!(parse_markers(&out.stdout, 1), [outcome(Some(0), "")]);
}

#[test]
fn step_line_form() {
    let script = build_script(&[
        step(Path::new("/usr/bin/rm"), &[b"-f", b"--", b"/var/x y/a'b"]),
        step(Path::new("/usr/bin/true"), &[]),
        step(Path::new("/usr/bin/rm"), &[b"-f", b"--", b"/var/x y/c"]),
        step(Path::new("/usr/bin/rm"), &[b"-f", b"--", b"/var/z"]),
        step(Path::new("/usr/bin/echo"), &[b"plain"]),
    ])
    .unwrap();
    let body = &script[HEADER.len()..];
    let want = "__t0() { __c \"$1\" '/usr/bin/rm' '-f' '--' '/var/x y/'\"$2\"; }\n\
                    __t0 0 'a'\\''b'\n\
                    __c 1 '/usr/bin/true'\n\
                    __t0 2 'c'\n\
                    __t1() { __c \"$1\" '/usr/bin/rm' '-f' '--' '/var/'\"$2\"; }\n\
                    __t1 3 'z'\n\
                    __t2() { __c \"$1\" '/usr/bin/echo' \"$2\"; }\n\
                    __t2 4 'plain'\n";
    assert_eq!(String::from_utf8_lossy(body), want);
}

#[test]
fn templated_steps_get_their_exact_args() {
    let t = FixtureTree::new();
    let out = t.dir.path().join("argv");
    let body = [
        b"#!/bin/sh\nfor a in \"$@\"; do printf '%s\\0' \"$a\"; done >> ".as_slice(),
        &sh_quote(out.as_os_str()),
        b"\n",
    ]
    .concat();
    let prog = t.write_exec("/usr/bin/argv-dump", &body);
    let outcomes = run(&[
        step(&prog, &[b"-f", b"/d i r/a'b $(x)"]),
        step(&prog, &[b"-f", b"/d i r/"]),
        step(&prog, &[b"-f", b"no-slash"]),
        step(&prog, &[b"-f", b"/d i r/\xff\n"]),
        step(&prog, &[]),
    ]);
    assert_eq!(outcomes, vec![outcome(Some(0), ""); 5]);
    let got = std::fs::read(&out).unwrap();
    assert_eq!(
        got,
        b"-f\0/d i r/a'b $(x)\0-f\0/d i r/\0-f\0no-slash\0-f\0/d i r/\xff\n\0"
    );
}

#[test]
fn a_thousand_core_dumps_fit() {
    // Core dump paths as systemd-coredump names them (~110 bytes).
    let steps: Vec<Step> = (0..1000)
        .map(|k| {
            let path = format!(
                "/var/lib/systemd/coredump/core.chrome_crashpad.1000.\
                     215a3aff80554b6dbe10ed300f45e4a7.{:07}.1793828890000000.zst",
                k
            );
            Step {
                program: PathBuf::from("/usr/bin/rm"),
                args: vec!["-f".into(), "--".into(), path.into()],
            }
        })
        .collect();
    let script = build_script(&steps).unwrap();
    assert!(script.len() <= MAX_SCRIPT_BYTES, "{}", script.len());
}

#[test]
fn a_step_cannot_redefine_the_step_body() {
    let t = FixtureTree::new();
    // Tries to replace `__c` and the template for the steps after it;
    // the step is its own process, so it cannot.
    let hijack = t.write_exec(
        "/usr/bin/hijack",
        b"#!/bin/sh\n__c() { echo '@@crumbs 1 0'; }\n__t0() { :; }\nexit 4\n",
    );
    let fail = t.write_exec("/usr/bin/fail", b"#!/bin/sh\nexit 6\n");
    let outcomes = run(&[step(&hijack, &[b"x"]), step(&fail, &[b"x"])]);
    assert_eq!(outcomes, [outcome(Some(4), ""), outcome(Some(6), "")]);
}
