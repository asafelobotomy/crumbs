//! The single root shell script for a run, its runner, and the parsing of
//! the per-step status markers it prints.
//!
//! The script is the only text crumbs hands to a root shell, so every
//! program path and argument goes through [`sh_quote`], and it is passed as
//! one argv element to `/bin/sh -c` (never through a file or stdin).

use std::ffi::{OsStr, OsString};
use std::io::Read;
use std::os::unix::ffi::OsStrExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::thread;

use crate::command::{CHILD_PATH, ENV_ALLOWLIST};
use crate::error::CoreError;
use crate::privilege::{Escalation, EscalationKind};

/// First line of every script: a fixed `PATH` and locale, and no preloads.
const HEADER: &[u8] =
    b"export PATH=/usr/bin:/usr/sbin:/bin:/sbin LC_ALL=C; unset LD_PRELOAD LD_LIBRARY_PATH\n";
const MARKER_PREFIX: &[u8] = b"@@crumbs ";
/// Extra variables the escalation program needs to show its prompt.
const DISPLAY_VARS: [&str; 3] = ["DISPLAY", "WAYLAND_DISPLAY", "XAUTHORITY"];
const SHELL: &str = "/bin/sh";

/// Wraps `s` in single quotes, writing each `'` as `'\''`. Works on raw
/// bytes, so non-UTF-8 arguments pass through unchanged.
pub fn sh_quote(s: &OsStr) -> Vec<u8> {
    let mut out = Vec::with_capacity(s.len() + 2);
    out.push(b'\'');
    for &b in s.as_bytes() {
        if b == b'\'' {
            out.extend_from_slice(b"'\\''");
        } else {
            out.push(b);
        }
    }
    out.push(b'\'');
    out
}

/// One command in the batch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Step {
    /// Absolute path written into the script as-is. Production callers pass
    /// the in-system path (`env.unsys` of a `resolve_trusted` result); tests
    /// running the script as the user pass root-joined fixture paths.
    pub program: PathBuf,
    pub args: Vec<OsString>,
}

/// Builds the batch: the header, then one line per step that runs it with
/// stdin from `/dev/null`, stderr folded into stdout, and prints a newline
/// then `@@crumbs <i> <status>` after it. The leading newline puts the marker
/// on its own line even when the step's output does not end in one. There is no `set -e`; a failing step
/// does not stop the next.
pub fn build_script(steps: &[Step]) -> Vec<u8> {
    let mut script = HEADER.to_vec();
    for (i, step) in steps.iter().enumerate() {
        script.extend(sh_quote(step.program.as_os_str()));
        for arg in &step.args {
            script.push(b' ');
            script.extend(sh_quote(arg));
        }
        script.extend_from_slice(b" </dev/null 2>&1; printf '\\n@@crumbs %d %d\\n' ");
        script.extend_from_slice(i.to_string().as_bytes());
        script.extend_from_slice(b" \"$?\"\n");
    }
    script
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StepOutcome {
    /// The step's exit status, or `None` when its marker never appeared (the
    /// shell died, or escalation was cancelled or denied).
    pub status: Option<i32>,
    /// What the step printed (stdout and stderr), decoded lossily.
    pub output: String,
}

/// Parses `@@crumbs <index> <status>` lines from `script`'s stdout.
///
/// A line is a marker only if it is exactly that form (ASCII digits, single
/// spaces), and its index is the next one expected; anything else, forged or
/// out-of-order markers included, is output of the step that is running.
/// Once a marker is missing, that step and every later one get `None`.
///
/// Each step's output loses exactly one trailing newline: the separator the
/// script prints before the marker.
pub fn parse_markers(stdout: &[u8], steps: usize) -> Vec<StepOutcome> {
    let mut outcomes = Vec::with_capacity(steps);
    let mut output = Vec::new();
    for line in stdout.split_inclusive(|&b| b == b'\n') {
        let next = outcomes.len();
        match marker(line) {
            Some((index, status)) if index == next && next < steps => {
                if output.last() == Some(&b'\n') {
                    output.pop();
                }
                outcomes.push(StepOutcome {
                    status: Some(status),
                    output: String::from_utf8_lossy(&output).into_owned(),
                });
                output.clear();
            }
            _ => output.extend_from_slice(line),
        }
    }
    if outcomes.len() < steps {
        outcomes.push(StepOutcome {
            status: None,
            output: String::from_utf8_lossy(&output).into_owned(),
        });
    }
    outcomes.resize(
        steps,
        StepOutcome {
            status: None,
            output: String::new(),
        },
    );
    outcomes
}

/// Reads a `@@crumbs <digits> <digits>\n` line.
fn marker(line: &[u8]) -> Option<(usize, i32)> {
    let rest = line.strip_prefix(MARKER_PREFIX)?.strip_suffix(b"\n")?;
    let mut parts = rest.split(|&b| b == b' ');
    let index = digits(parts.next()?)?;
    let status = digits(parts.next()?)?;
    if parts.next().is_some() {
        return None;
    }
    Some((index.parse().ok()?, status.parse().ok()?))
}

fn digits(b: &[u8]) -> Option<&str> {
    if b.is_empty() || !b.iter().all(u8::is_ascii_digit) {
        return None;
    }
    std::str::from_utf8(b).ok()
}

/// Runs a batch script as root and returns its stdout.
pub trait PrivilegedRunner: Send + Sync {
    fn run(&self, esc: &Escalation, script: &[u8]) -> Result<Vec<u8>, CoreError>;
}

/// Runs `<esc.program> [--] /bin/sh -c <script>` for real.
///
/// stdin and stderr are the terminal's, so run0, pkexec and sudo can prompt;
/// stdout is captured. The child stays in crumbs' process group (the
/// escalation program must own the terminal for its prompt), so Ctrl-C
/// reaches it too. There is no timeout: the user may take a while to answer.
pub struct SystemPrivilegedRunner;

/// The escalation program's arguments for running `script` in a root shell.
fn escalation_args(esc: &Escalation, script: &[u8]) -> Vec<OsString> {
    let mut args = Vec::with_capacity(4);
    match esc.kind {
        EscalationKind::Run0 | EscalationKind::Sudo => args.push(OsString::from("--")),
        EscalationKind::Pkexec => {}
    }
    args.push(OsString::from(SHELL));
    args.push(OsString::from("-c"));
    args.push(OsStr::from_bytes(script).to_os_string());
    args
}

impl PrivilegedRunner for SystemPrivilegedRunner {
    fn run(&self, esc: &Escalation, script: &[u8]) -> Result<Vec<u8>, CoreError> {
        let fail = |e: std::io::Error| CoreError::Command {
            program: esc.program.display().to_string(),
            message: e.to_string(),
        };
        let mut cmd = Command::new(&esc.program);
        cmd.args(escalation_args(esc, script)).env_clear();
        for (k, v) in std::env::vars_os() {
            if ENV_ALLOWLIST.iter().chain(&DISPLAY_VARS).any(|a| k == *a) {
                cmd.env(k, v);
            }
        }
        cmd.env("PATH", CHILD_PATH).env("LC_ALL", "C");
        let mut child = cmd
            .stdin(Stdio::inherit())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(fail)?;
        let reader = child.stdout.take().map(|mut out| {
            thread::spawn(move || {
                let mut buf = Vec::new();
                let _ = out.read_to_end(&mut buf);
                buf
            })
        });
        // The exit status is not used: the markers say what ran.
        child.wait().map_err(fail)?;
        Ok(reader.and_then(|h| h.join().ok()).unwrap_or_default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::privilege::EscalationKind;
    use crate::testing::{FakePrivilegedRunner, FixtureTree};
    use std::os::unix::ffi::{OsStrExt, OsStringExt};
    use std::process::Command;

    fn esc() -> Escalation {
        Escalation {
            kind: EscalationKind::Run0,
            program: PathBuf::from("/usr/bin/run0"),
            requires_tty: false,
        }
    }

    fn step(program: &std::path::Path, args: &[&[u8]]) -> Step {
        Step {
            program: program.to_path_buf(),
            args: args
                .iter()
                .map(|a| OsString::from_vec(a.to_vec()))
                .collect(),
        }
    }

    fn run(steps: &[Step]) -> Vec<StepOutcome> {
        let script = build_script(steps);
        let stdout = FakePrivilegedRunner::default()
            .run(&esc(), &script)
            .unwrap();
        parse_markers(&stdout, steps.len())
    }

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
        assert_eq!(outcomes.len(), 1);
        assert_eq!(outcomes[0].status, Some(0), "{outcomes:?}");
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
                StepOutcome {
                    status: Some(0),
                    output: "first\nmore\n".into()
                },
                StepOutcome {
                    status: Some(3),
                    output: "oops\n".into()
                },
                StepOutcome {
                    status: Some(0),
                    output: "third\nmore\n".into()
                },
            ]
        );
    }

    #[test]
    fn missing_marker_is_none() {
        let t = FixtureTree::new();
        let ok = t.write_exec("/usr/bin/ok", b"#!/bin/sh\necho fine\n");
        // Kills the batch shell (its parent) before it can print a marker.
        let killer = t.write_exec(
            "/usr/bin/killer",
            b"#!/bin/sh\necho dying\nkill -KILL $PPID\n",
        );
        let outcomes = run(&[step(&ok, &[]), step(&killer, &[]), step(&ok, &[])]);
        assert_eq!(
            outcomes,
            [
                StepOutcome {
                    status: Some(0),
                    output: "fine\n".into()
                },
                StepOutcome {
                    status: None,
                    output: "dying\n".into()
                },
                StepOutcome {
                    status: None,
                    output: String::new()
                },
            ]
        );
    }

    #[test]
    fn forged_marker_in_output_ignored() {
        let t = FixtureTree::new();
        let forger = t.write_exec(
            "/usr/bin/forger",
            b"#!/bin/sh\necho '@@crumbs 7 0'\necho '@@crumbs 1 0'\necho '@@crumbs 0 x'\nexit 4\n",
        );
        let ok = t.write_exec("/usr/bin/ok", b"#!/bin/sh\n");
        let outcomes = run(&[step(&forger, &[]), step(&ok, &[])]);
        assert_eq!(outcomes.len(), 2);
        assert_eq!(outcomes[0].status, Some(4));
        assert_eq!(
            outcomes[0].output,
            "@@crumbs 7 0\n@@crumbs 1 0\n@@crumbs 0 x\n"
        );
        assert_eq!(outcomes[1].status, Some(0));
        assert_eq!(outcomes[1].output, "");
    }

    #[test]
    fn markers_must_be_in_order() {
        let outcomes = parse_markers(b"@@crumbs 1 0\n\n@@crumbs 0 2\n@@crumbs 0 0\n", 2);
        assert_eq!(outcomes[0].status, Some(2));
        assert_eq!(outcomes[0].output, "@@crumbs 1 0\n");
        // The duplicate marker for step 0 is output of step 1, which never
        // finished.
        assert_eq!(outcomes[1].status, None);
        assert_eq!(outcomes[1].output, "@@crumbs 0 0\n");
        // Signs, spaces and overflow are not markers.
        let outcomes = parse_markers(b"@@crumbs +0 0\n@@crumbs 0  0\n@@crumbs 0 99999999999\n", 1);
        assert_eq!(outcomes[0].status, None);
        assert_eq!(parse_markers(b"", 0), []);
    }

    #[test]
    fn marker_found_after_output_without_newline() {
        let t = FixtureTree::new();
        let bare = t.write_exec("/usr/bin/bare", b"#!/bin/sh\nprintf foo\nexit 5\n");
        let line = t.write_exec("/usr/bin/line", b"#!/bin/sh\nprintf 'foo\\n'\n");
        let quiet = t.write_exec("/usr/bin/quiet", b"#!/bin/sh\n");
        let outcomes = run(&[step(&bare, &[]), step(&line, &[]), step(&quiet, &[])]);
        assert_eq!(
            outcomes,
            [
                StepOutcome {
                    status: Some(5),
                    output: "foo".into()
                },
                // Only the separator newline is dropped; the program's own
                // line ending stays, so there is no extra blank line.
                StepOutcome {
                    status: Some(0),
                    output: "foo\n".into()
                },
                StepOutcome {
                    status: Some(0),
                    output: String::new()
                },
            ]
        );
    }

    #[test]
    fn header_clears_preload() {
        let t = FixtureTree::new();
        let script = build_script(&[step(std::path::Path::new("/usr/bin/env"), &[])]);
        assert!(script.starts_with(
            b"export PATH=/usr/bin:/usr/sbin:/bin:/sbin LC_ALL=C; unset LD_PRELOAD LD_LIBRARY_PATH\n"
        ));
        // A hostile inherited environment does not reach the steps.
        let out = Command::new("/bin/sh")
            .arg("-c")
            .arg(OsStr::from_bytes(&script))
            .env("LD_PRELOAD", "")
            .env("LD_LIBRARY_PATH", t.dir.path())
            .env("PATH", t.dir.path())
            .env("LC_ALL", "de_DE.UTF-8")
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
    fn step_line_form() {
        let script = build_script(&[
            step(
                std::path::Path::new("/usr/bin/rm"),
                &[b"-f", b"--", b"/var/x y"],
            ),
            step(std::path::Path::new("/usr/bin/true"), &[]),
        ]);
        let body = &script[script.iter().position(|&b| b == b'\n').unwrap() + 1..];
        assert_eq!(
            body,
            b"'/usr/bin/rm' '-f' '--' '/var/x y' </dev/null 2>&1; printf '\\n@@crumbs %d %d\\n' 0 \"$?\"\n\
              '/usr/bin/true' </dev/null 2>&1; printf '\\n@@crumbs %d %d\\n' 1 \"$?\"\n"
        );
    }

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
    fn fake_runner_records_scripts() {
        let r = FakePrivilegedRunner::default();
        r.run(&esc(), b"echo one").unwrap();
        r.run(&esc(), b"echo two").unwrap();
        assert_eq!(
            *r.calls.lock().unwrap(),
            [b"echo one".to_vec(), b"echo two".to_vec()]
        );
    }
}
