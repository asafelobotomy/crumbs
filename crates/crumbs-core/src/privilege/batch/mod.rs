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

mod runner;
pub use runner::*;
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

#[cfg(test)]
mod tests;
