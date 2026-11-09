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
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;

use crate::command::{CHILD_PATH, ENV_ALLOWLIST};
use crate::error::CoreError;
use crate::privilege::{Escalation, EscalationKind, Interaction};

/// Start of every script: a fixed `PATH` and locale, no preloads, no
/// inherited `IFS`/`CDPATH`, and no functions shadowing the builtins the
/// script uses (`command unset` keeps bash in POSIX mode (`/bin/sh`) from
/// exiting on the readonly `BASHOPTS`). Then, defined only after those
/// unsets, `__c <i> <program> <arg>…`, the body of every step: it runs the
/// program with stdin from `/dev/null` and stderr folded into stdout,
/// re-emits the captured output with each line prefixed by `|` (empty
/// output emits nothing), and prints `@@crumbs <i> <status>`. The program
/// is an absolute path, so it is never a function or builtin, and it runs
/// in a `$( )` subshell as its own process: nothing a step does can
/// redefine `__c` or the step templates for the steps after it.
const HEADER: &[u8] = b"export PATH=/usr/bin:/usr/sbin:/bin:/sbin LC_ALL=C; \
unset LD_PRELOAD LD_LIBRARY_PATH; \
unset -f printf read command 2>/dev/null || true; \
command unset IFS CDPATH BASHOPTS 2>/dev/null || true\n\
__c() { __i=$1; shift; __o=$(\"$@\" </dev/null 2>&1); __s=$?; \
case $__o in '') ;; *) \
command printf '%s\\n' \"$__o\" | while IFS= read -r __l; do \
command printf '|%s\\n' \"$__l\"; done ;; esac; \
command printf '@@crumbs %d %d\\n' \"$__i\" \"$__s\"; }\n";
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
    /// `bytes` is the script's size when it went past the limit.
    #[error("too many items for one privileged run; select fewer items with -i")]
    TooLarge { bytes: usize },
}

/// One command in the batch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Step {
    /// Absolute path written into the script as-is. The executor passes the
    /// `resolve_trusted` result: root-joined, so the in-system path on the
    /// real system (root `/`) and the fixture program in tests, which run
    /// the script as the user.
    pub program: PathBuf,
    pub args: Vec<OsString>,
}

/// Builds the batch: the header, then one line per step. Each step runs
/// with stdin from `/dev/null` and stderr folded into stdout; its output is
/// captured, re-emitted with a `|` before every line (trailing newlines are
/// dropped), and followed by `@@crumbs <i> <status>`. There is no `set -e`;
/// a failing step does not stop the next.
///
/// Steps that share their program, every argument but the last, and the
/// last argument's directory (everything up to its last `/`) share a
/// template, `__t<n>() { __c "$1" <program> <args…> <dir>"$2"; }`, defined
/// just before its first use, so each step line is `__t<n> <i> <rest>`:
/// many deletions in one directory stay small.
pub fn build_script(steps: &[Step]) -> Result<Vec<u8>, BatchError> {
    let mut script = HEADER.to_vec();
    // The quoted template body (program, leading args, dir) of each
    // template defined so far; its index is its number.
    let mut templates: Vec<Vec<u8>> = Vec::new();
    for (i, step) in steps.iter().enumerate() {
        if !step.program.is_absolute() {
            return Err(BatchError::RelativeProgram(step.program.clone()));
        }
        let mut body = quote_checked(step.program.as_os_str())?;
        let Some((last, leading)) = step.args.split_last() else {
            script.extend_from_slice(format!("__c {i} ").as_bytes());
            script.extend(body);
            script.push(b'\n');
            check_size(&script)?;
            continue;
        };
        for arg in leading {
            body.push(b' ');
            body.extend(quote_checked(arg)?);
        }
        let last = last.as_bytes();
        if last.contains(&0) {
            return Err(BatchError::InvalidArgument(
                String::from_utf8_lossy(last).into_owned(),
            ));
        }
        let cut = last.iter().rposition(|&b| b == b'/').map_or(0, |p| p + 1);
        let (dir, rest) = last.split_at(cut);
        body.push(b' ');
        if !dir.is_empty() {
            body.extend(sh_quote(OsStr::from_bytes(dir)));
        }
        let n = match templates.iter().position(|t| *t == body) {
            Some(n) => n,
            None => {
                let n = templates.len();
                script.extend_from_slice(format!("__t{n}() {{ __c \"$1\" ").as_bytes());
                script.extend_from_slice(&body);
                script.extend_from_slice(b"\"$2\"; }\n");
                templates.push(body);
                n
            }
        };
        script.extend_from_slice(format!("__t{n} {i} ").as_bytes());
        script.extend(sh_quote(OsStr::from_bytes(rest)));
        script.push(b'\n');
        check_size(&script)?;
    }
    Ok(script)
}

/// Refuses a script past [`MAX_SCRIPT_BYTES`].
fn check_size(script: &[u8]) -> Result<(), BatchError> {
    if script.len() > MAX_SCRIPT_BYTES {
        return Err(BatchError::TooLarge {
            bytes: script.len(),
        });
    }
    Ok(())
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

/// What a privileged run wrote: the script's stdout and, when the runner
/// captures it (see [`Interaction::NoTerminal`]), the escalation's stderr.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PrivilegedRun {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

/// Runs a batch script as root and returns its stdout.
pub trait PrivilegedRunner: Send + Sync {
    fn run(&self, esc: &Escalation, script: &[u8]) -> Result<Vec<u8>, CoreError>;

    /// Like [`run`](Self::run), also returning captured stderr. Runners that
    /// do not capture it return it empty.
    fn run_captured(&self, esc: &Escalation, script: &[u8]) -> Result<PrivilegedRun, CoreError> {
        Ok(PrivilegedRun {
            stdout: self.run(esc, script)?,
            stderr: Vec::new(),
        })
    }

    /// Whether this runner has a terminal; callers needing no terminal can
    /// check it.
    fn interaction(&self) -> Interaction {
        Interaction::Terminal
    }
}

/// Most captured stderr kept; the tail, since the end says why it failed.
pub const MAX_STDERR_BYTES: usize = 64 * 1024;
const TRUNCATED: &[u8] = "… (truncated)\n".as_bytes();

/// Keeps the last [`MAX_STDERR_BYTES`] of `buf`, prefixed with
/// `… (truncated)\n` when it was cut.
pub fn cap_stderr(mut buf: Vec<u8>) -> Vec<u8> {
    if buf.len() <= MAX_STDERR_BYTES {
        return buf;
    }
    let mut out = TRUNCATED.to_vec();
    out.extend(buf.drain(buf.len() - MAX_STDERR_BYTES..));
    out
}

/// How one standard stream of the escalation program is set up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StdioMode {
    Inherit,
    Null,
    Piped,
}

impl StdioMode {
    fn stdio(self) -> Stdio {
        match self {
            StdioMode::Inherit => Stdio::inherit(),
            StdioMode::Null => Stdio::null(),
            StdioMode::Piped => Stdio::piped(),
        }
    }
}

/// The escalation program's `(stdin, stderr)` for `i`: the terminal's for
/// [`Interaction::Terminal`]; `/dev/null` and a pipe otherwise.
pub fn stdio_modes(i: Interaction) -> (StdioMode, StdioMode) {
    match i {
        Interaction::Terminal => (StdioMode::Inherit, StdioMode::Inherit),
        Interaction::NoTerminal => (StdioMode::Null, StdioMode::Piped),
    }
}

/// Runs `<esc.program> [--] /bin/sh -c <script>` for real.
///
/// With [`Interaction::Terminal`], stdin and stderr are the terminal's, so
/// run0, pkexec and sudo can prompt; stdout is captured. With
/// [`Interaction::NoTerminal`] stdin is `/dev/null` and stderr is captured
/// (returned by `run_captured`, never printed). The child stays in crumbs' process group (the
/// escalation program must own the terminal for its prompt), so Ctrl-C
/// reaches it too. There is no timeout: the user may take a while to answer.
///
/// It refuses, without starting anything, inside crumbs' own unit tests and
/// whenever the environment's root is not `/`: under a fake root the
/// programs in the script are user-writable fixtures.
pub struct SystemPrivilegedRunner {
    root: PathBuf,
    interaction: Interaction,
}

impl SystemPrivilegedRunner {
    /// `root` is the environment's root (`Env::root`).
    pub fn new(root: &Path) -> Self {
        Self::with_interaction(root, Interaction::Terminal)
    }

    pub fn with_interaction(root: &Path, i: Interaction) -> Self {
        SystemPrivilegedRunner {
            root: root.to_path_buf(),
            interaction: i,
        }
    }
}

/// Why escalating is refused: inside tests, or under a fake root.
fn escalation_refused(root: &Path, in_test: bool) -> Option<&'static str> {
    if in_test {
        Some("refusing to escalate inside tests")
    } else if root != Path::new("/") {
        Some("refusing to escalate under a fake root")
    } else {
        None
    }
}

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
        self.run_captured(esc, script).map(|r| r.stdout)
    }

    fn interaction(&self) -> Interaction {
        self.interaction
    }

    fn run_captured(&self, esc: &Escalation, script: &[u8]) -> Result<PrivilegedRun, CoreError> {
        if let Some(why) = escalation_refused(&self.root, cfg!(test)) {
            return Err(CoreError::Command {
                program: esc.program.display().to_string(),
                message: why.into(),
            });
        }
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
        let (stdin, stderr) = stdio_modes(self.interaction);
        let mut child = cmd
            .stdin(stdin.stdio())
            .stdout(Stdio::piped())
            .stderr(stderr.stdio())
            .spawn()
            .map_err(fail)?;
        let drain = |r: Option<Box<dyn Read + Send>>| {
            r.map(|mut r| {
                thread::spawn(move || {
                    let mut buf = Vec::new();
                    let _ = r.read_to_end(&mut buf);
                    buf
                })
            })
        };
        let out = drain(child.stdout.take().map(|s| Box::new(s) as _));
        let err = drain(child.stderr.take().map(|s| Box::new(s) as _));
        // The exit status is not used: the markers say what ran.
        child.wait().map_err(fail)?;
        let join = |h: Option<thread::JoinHandle<Vec<u8>>>| {
            h.and_then(|h| h.join().ok()).unwrap_or_default()
        };
        Ok(PrivilegedRun {
            stdout: join(out),
            stderr: cap_stderr(join(err)),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::privilege::EscalationKind;
    use crate::testing::{FakePrivilegedRunner, FixtureTree};
    use std::os::unix::ffi::OsStringExt;

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
}
