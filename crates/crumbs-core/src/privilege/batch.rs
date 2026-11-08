//! The single root shell script for a run, its runner, and the parsing of
//! the per-step status markers it prints.
//!
//! The script is the only text crumbs hands to a root shell, so every
//! program path and argument goes through [`sh_quote`], and it is passed as
//! one argv element to `/bin/sh -c` (never through a file or stdin).
//!
//! Each step's combined output is captured by the shell and re-emitted with
//! every line prefixed by `|`, so nothing a step prints can begin with the
//! `@@crumbs` marker: only the script itself writes markers.

use std::ffi::{OsStr, OsString};
use std::io::Read;
use std::os::unix::ffi::OsStrExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::thread;

use crate::command::{CHILD_PATH, ENV_ALLOWLIST};
use crate::error::CoreError;
use crate::privilege::{Escalation, EscalationKind};

/// First line of every script: a fixed `PATH` and locale, no preloads, no
/// inherited `IFS`/`CDPATH`, and no functions shadowing the builtins the
/// script uses. `command unset` keeps bash in POSIX mode (`/bin/sh`) from
/// exiting on the readonly `BASHOPTS`.
const HEADER: &[u8] = b"export PATH=/usr/bin:/usr/sbin:/bin:/sbin LC_ALL=C; \
unset LD_PRELOAD LD_LIBRARY_PATH; \
unset -f printf read command 2>/dev/null || true; \
command unset IFS CDPATH BASHOPTS 2>/dev/null || true\n";
/// Runs after `__o=$(<step>` and re-emits the captured output, each line
/// prefixed with `|`; empty output emits nothing.
const STEP_TAIL: &[u8] = b" </dev/null 2>&1); __s=$?; case $__o in '') ;; *) \
command printf '%s\\n' \"$__o\" | while IFS= read -r __l; do \
command printf '|%s\\n' \"$__l\"; done ;; esac; \
command printf '@@crumbs %d %d\\n' ";
const OUTPUT_PREFIX: u8 = b'|';
/// Largest script accepted; Linux limits one argv string to 128 KiB.
const MAX_SCRIPT_BYTES: usize = 120 * 1024;
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

/// Why a batch script could not be built.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BatchError {
    #[error("argument contains a NUL byte: {0}")]
    InvalidArgument(String),
    #[error("program path is not absolute: {}", .0.display())]
    RelativeProgram(PathBuf),
    #[error(
        "too many items for one privileged run ({bytes} bytes); clean fewer categories at once"
    )]
    TooLarge { bytes: usize },
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

/// Builds the batch: the header, then one line per step. Each step runs
/// with stdin from `/dev/null` and stderr folded into stdout; its output is
/// captured, re-emitted with a `|` before every line (trailing newlines are
/// dropped), and followed by `@@crumbs <i> <status>`. There is no `set -e`;
/// a failing step does not stop the next.
pub fn build_script(steps: &[Step]) -> Result<Vec<u8>, BatchError> {
    let mut script = HEADER.to_vec();
    for (i, step) in steps.iter().enumerate() {
        if !step.program.is_absolute() {
            return Err(BatchError::RelativeProgram(step.program.clone()));
        }
        script.extend_from_slice(b"__o=$(");
        script.extend(quote_checked(step.program.as_os_str())?);
        for arg in &step.args {
            script.push(b' ');
            script.extend(quote_checked(arg)?);
        }
        script.extend_from_slice(STEP_TAIL);
        script.extend_from_slice(i.to_string().as_bytes());
        script.extend_from_slice(b" \"$__s\"\n");
        if script.len() > MAX_SCRIPT_BYTES {
            return Err(BatchError::TooLarge {
                bytes: script.len(),
            });
        }
    }
    Ok(script)
}

/// Quotes `s`, refusing a NUL byte (it cannot be passed in argv).
fn quote_checked(s: &OsStr) -> Result<Vec<u8>, BatchError> {
    if s.as_bytes().contains(&0) {
        return Err(BatchError::InvalidArgument(
            s.to_string_lossy().into_owned(),
        ));
    }
    Ok(sh_quote(s))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StepOutcome {
    /// The step's exit status, or `None` when its marker never appeared (the
    /// shell died, or escalation was cancelled or denied).
    pub status: Option<i32>,
    /// What the step printed (stdout and stderr), decoded lossily.
    pub output: String,
}

/// Parses a batch's stdout into one outcome per step.
///
/// A line starting with `|` is output of the step that is running (the `|`
/// is removed and the lines are joined with `\n`). A line that is exactly
/// `@@crumbs <index> <status>` (ASCII digits, single spaces) with the next
/// expected index ends that step. Every other line is ignored. Once a marker
/// is missing, that step and every later one get `None`.
pub fn parse_markers(stdout: &[u8], steps: usize) -> Vec<StepOutcome> {
    let mut outcomes = Vec::with_capacity(steps);
    let mut lines: Vec<&[u8]> = Vec::new();
    let take = |lines: &mut Vec<&[u8]>| {
        let joined = lines.join(&b'\n');
        lines.clear();
        String::from_utf8_lossy(&joined).into_owned()
    };
    for line in stdout.split_inclusive(|&b| b == b'\n') {
        if let Some(text) = line.strip_prefix(&[OUTPUT_PREFIX]) {
            lines.push(text.strip_suffix(b"\n").unwrap_or(text));
            continue;
        }
        let next = outcomes.len();
        if let Some((index, status)) = marker(line)
            && index == next
            && next < steps
        {
            outcomes.push(StepOutcome {
                status: Some(status),
                output: take(&mut lines),
            });
        }
    }
    if outcomes.len() < steps {
        outcomes.push(StepOutcome {
            status: None,
            output: take(&mut lines),
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
    use std::os::unix::ffi::OsStringExt;
    use std::path::Path;

    fn esc() -> Escalation {
        Escalation {
            kind: EscalationKind::Run0,
            program: PathBuf::from("/usr/bin/run0"),
            requires_tty: false,
        }
    }

    fn step(program: &Path, args: &[&[u8]]) -> Step {
        Step {
            program: program.to_path_buf(),
            args: args
                .iter()
                .map(|a| OsString::from_vec(a.to_vec()))
                .collect(),
        }
    }

    fn run(steps: &[Step]) -> Vec<StepOutcome> {
        let script = build_script(steps).unwrap();
        let stdout = FakePrivilegedRunner::default()
            .run(&esc(), &script)
            .unwrap();
        parse_markers(&stdout, steps.len())
    }

    fn outcome(status: Option<i32>, output: &str) -> StepOutcome {
        StepOutcome {
            status,
            output: output.into(),
        }
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
            format!(
                "too many items for one privileged run ({bytes} bytes); clean fewer categories at once"
            )
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
            step(Path::new("/usr/bin/rm"), &[b"-f", b"--", b"/var/x y"]),
            step(Path::new("/usr/bin/true"), &[]),
        ])
        .unwrap();
        let body = &script[HEADER.len()..];
        let tail: &[u8] = b" </dev/null 2>&1); __s=$?; case $__o in '') ;; *) \
            command printf '%s\\n' \"$__o\" | while IFS= read -r __l; do \
            command printf '|%s\\n' \"$__l\"; done ;; esac; \
            command printf '@@crumbs %d %d\\n' ";
        let want = [
            b"__o=$('/usr/bin/rm' '-f' '--' '/var/x y'".as_slice(),
            tail,
            b"0 \"$__s\"\n",
            b"__o=$('/usr/bin/true'",
            tail,
            b"1 \"$__s\"\n",
        ]
        .concat();
        assert_eq!(
            String::from_utf8_lossy(body),
            String::from_utf8_lossy(&want)
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
