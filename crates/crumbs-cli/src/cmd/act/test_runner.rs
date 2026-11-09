//! Debug-only stand-ins for the escalation, selected with
//! `CRUMBS_TEST_PRIV_RUNNER`.

use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crumbs_core::CoreError;
use crumbs_core::privilege::batch::{PrivilegedRun, PrivilegedRunner, cap_stderr};
use crumbs_core::privilege::{Escalation, EscalationKind, Interaction};

/// Runs the script with `/bin/sh` as the current user. Stdin is always
/// `/dev/null`; under `NoTerminal` the script's stderr is captured and
/// returned instead of dropped. Either way it always prints the
/// `crumbs-test: privileged batch` line to the process's own stderr (a
/// server's stderr is its log, never the protocol), and then
/// `crumbs-test: batch stdin <path>`, where `<path>` is what the batch's
/// shell itself found on its stdin.
struct Sh(Interaction);

/// The line the batch's shell writes first, on its stderr: what its stdin is.
const STDIN_PROBE: &[u8] =
    b"printf 'crumbs-test: batch stdin %s\\n' \"$(readlink /proc/self/fd/0)\" >&2\n";
const STDIN_PREFIX: &[u8] = b"crumbs-test: batch stdin ";

impl PrivilegedRunner for Sh {
    fn run(&self, esc: &Escalation, script: &[u8]) -> Result<Vec<u8>, CoreError> {
        self.run_captured(esc, script).map(|r| r.stdout)
    }

    fn interaction(&self) -> Interaction {
        self.0
    }

    fn run_captured(&self, _esc: &Escalation, script: &[u8]) -> Result<PrivilegedRun, CoreError> {
        // Lets tests count the batches of a run.
        eprintln!("crumbs-test: privileged batch");
        let capture = self.0 == Interaction::NoTerminal;
        let mut probed = STDIN_PROBE.to_vec();
        probed.extend_from_slice(script);
        let out = Command::new("/bin/sh")
            .arg("-c")
            .arg(OsStr::from_bytes(&probed))
            .stdin(Stdio::null())
            .stderr(Stdio::piped())
            .output()
            .map_err(|e| CoreError::Command {
                program: "/bin/sh".into(),
                message: e.to_string(),
            })?;
        // The probe's line goes to this process's stderr; the rest is the
        // batch's own, kept only when captured.
        let mut stderr = Vec::new();
        for line in out.stderr.split_inclusive(|&b| b == b'\n') {
            if line.starts_with(STDIN_PREFIX) {
                eprint!("{}", String::from_utf8_lossy(line));
            } else if capture {
                stderr.extend_from_slice(line);
            }
        }
        Ok(PrivilegedRun {
            stdout: out.stdout,
            stderr: cap_stderr(stderr),
        })
    }
}

/// [`Sh`] on the script cut just before step `n`'s line: the shell
/// ends after `n` steps, as if it had been killed during step `n`.
struct Partial(usize, Interaction);

impl PrivilegedRunner for Partial {
    fn run(&self, esc: &Escalation, script: &[u8]) -> Result<Vec<u8>, CoreError> {
        self.run_captured(esc, script).map(|r| r.stdout)
    }

    fn interaction(&self) -> Interaction {
        self.1
    }

    fn run_captured(&self, esc: &Escalation, script: &[u8]) -> Result<PrivilegedRun, CoreError> {
        let mut cut = Vec::new();
        for line in script.split_inclusive(|&b| b == b'\n') {
            if step_index(line) == Some(self.0) {
                break;
            }
            cut.extend_from_slice(line);
        }
        Sh(self.1).run_captured(esc, &cut)
    }
}

/// The index of a step line (`__c <i> …` or `__t<n> <i> …`).
fn step_index(line: &[u8]) -> Option<usize> {
    let mut words = line.split(|&b| b == b' ');
    let first = words.next()?;
    let is_step = first == b"__c"
        || first
            .strip_prefix(b"__t")
            .is_some_and(|n| !n.is_empty() && n.iter().all(u8::is_ascii_digit));
    if !is_step {
        return None;
    }
    std::str::from_utf8(words.next()?).ok()?.parse().ok()
}

/// Runs nothing and prints nothing, like a dismissed prompt.
struct Deny(Interaction);

impl PrivilegedRunner for Deny {
    fn run(&self, _esc: &Escalation, _script: &[u8]) -> Result<Vec<u8>, CoreError> {
        Ok(Vec::new())
    }

    fn interaction(&self) -> Interaction {
        self.0
    }
}

/// The stand-in that `value` (of `CRUMBS_TEST_PRIV_RUNNER`) names,
/// only when `root` is a fake root: on the real system the fixture
/// stand-ins would run real programs as the user.
pub(super) fn select(
    root: &Path,
    value: Option<&str>,
    i: Interaction,
) -> Option<Box<dyn PrivilegedRunner>> {
    if root == Path::new("/") {
        return None;
    }
    match value? {
        "sh" => Some(Box::new(Sh(i))),
        "deny" => Some(Box::new(Deny(i))),
        v => {
            let n = v.strip_prefix("partial:")?.parse().ok()?;
            Some(Box::new(Partial(n, i)))
        }
    }
}

pub(super) fn escalation() -> Escalation {
    Escalation {
        kind: EscalationKind::Run0,
        program: PathBuf::from("/usr/bin/run0"),
        requires_tty: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const T: Interaction = Interaction::Terminal;

    #[test]
    fn test_runners_report_the_interaction_and_sh_has_no_stdin() {
        let fake = std::path::Path::new("/tmp/fixture");
        let esc = escalation();
        for v in ["sh", "deny", "partial:1"] {
            let r = select(fake, Some(v), Interaction::NoTerminal).unwrap();
            assert_eq!(r.interaction(), Interaction::NoTerminal, "{v}");
            let r = select(fake, Some(v), T).unwrap();
            assert_eq!(r.interaction(), T, "{v}");
        }
        // stdin is /dev/null whatever the interaction: `cat` sees EOF, so a
        // read from it cannot block or take protocol bytes.
        let script = b"x=$(cat); echo \"in:[$x]\"; echo err >&2";
        for i in [T, Interaction::NoTerminal] {
            let run = Sh(i).run_captured(&esc, script).unwrap();
            assert_eq!(run.stdout, b"in:[]\n");
            // Captured only without a terminal.
            let want: &[u8] = if i == T { b"" } else { b"err\n" };
            assert_eq!(run.stderr, want);
        }
    }

    #[test]
    fn test_runner_only_under_a_fake_root() {
        let fake = std::path::Path::new("/tmp/fixture");
        assert!(select(std::path::Path::new("/"), Some("sh"), T).is_none());
        assert!(select(std::path::Path::new("/"), Some("deny"), T).is_none());
        assert!(select(fake, Some("sh"), T).is_some());
        assert!(select(fake, Some("deny"), T).is_some());
        assert!(select(fake, Some("other"), T).is_none());
        assert!(select(fake, Some("partial:2"), T).is_some());
        assert!(select(fake, Some("partial:x"), T).is_none());
        assert!(select(std::path::Path::new("/"), Some("partial:1"), T).is_none());
        assert!(select(fake, None, T).is_none());
    }

    #[test]
    fn step_index_reads_step_lines_only() {
        assert_eq!(step_index(b"__c 3 '/usr/bin/true'\n"), Some(3));
        assert_eq!(step_index(b"__t12 40 'x'\n"), Some(40));
        assert_eq!(step_index(b"__t0() { __c \"$1\" 'x' \"$2\"; }\n"), None);
        assert_eq!(step_index(b"__c() { __i=$1; }\n"), None);
        assert_eq!(step_index(b"export PATH=/usr/bin\n"), None);
    }
}
