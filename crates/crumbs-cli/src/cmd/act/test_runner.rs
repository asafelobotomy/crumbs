//! Debug-only stand-ins for the escalation, selected with
//! `CRUMBS_TEST_PRIV_RUNNER`.

use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crumbs_core::CoreError;
use crumbs_core::privilege::batch::PrivilegedRunner;
use crumbs_core::privilege::{Escalation, EscalationKind};

/// Runs the script with `/bin/sh` as the current user.
struct Sh;

impl PrivilegedRunner for Sh {
    fn run(&self, _esc: &Escalation, script: &[u8]) -> Result<Vec<u8>, CoreError> {
        // Lets tests count the batches of a run.
        eprintln!("crumbs-test: privileged batch");
        let out = Command::new("/bin/sh")
            .arg("-c")
            .arg(OsStr::from_bytes(script))
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .map_err(|e| CoreError::Command {
                program: "/bin/sh".into(),
                message: e.to_string(),
            })?;
        Ok(out.stdout)
    }
}

/// [`Sh`] on the script cut just before step `n`'s line: the shell
/// ends after `n` steps, as if it had been killed during step `n`.
struct Partial(usize);

impl PrivilegedRunner for Partial {
    fn run(&self, esc: &Escalation, script: &[u8]) -> Result<Vec<u8>, CoreError> {
        let mut cut = Vec::new();
        for line in script.split_inclusive(|&b| b == b'\n') {
            if step_index(line) == Some(self.0) {
                break;
            }
            cut.extend_from_slice(line);
        }
        Sh.run(esc, &cut)
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
struct Deny;

impl PrivilegedRunner for Deny {
    fn run(&self, _esc: &Escalation, _script: &[u8]) -> Result<Vec<u8>, CoreError> {
        Ok(Vec::new())
    }
}

/// The stand-in that `value` (of `CRUMBS_TEST_PRIV_RUNNER`) names,
/// only when `root` is a fake root: on the real system the fixture
/// stand-ins would run real programs as the user.
pub(super) fn select(root: &Path, value: Option<&str>) -> Option<Box<dyn PrivilegedRunner>> {
    if root == Path::new("/") {
        return None;
    }
    match value? {
        "sh" => Some(Box::new(Sh)),
        "deny" => Some(Box::new(Deny)),
        v => {
            let n = v.strip_prefix("partial:")?.parse().ok()?;
            Some(Box::new(Partial(n)))
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

    #[test]
    fn test_runner_only_under_a_fake_root() {
        let fake = std::path::Path::new("/tmp/fixture");
        assert!(select(std::path::Path::new("/"), Some("sh")).is_none());
        assert!(select(std::path::Path::new("/"), Some("deny")).is_none());
        assert!(select(fake, Some("sh")).is_some());
        assert!(select(fake, Some("deny")).is_some());
        assert!(select(fake, Some("other")).is_none());
        assert!(select(fake, Some("partial:2")).is_some());
        assert!(select(fake, Some("partial:x")).is_none());
        assert!(select(std::path::Path::new("/"), Some("partial:1")).is_none());
        assert!(select(fake, None).is_none());
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
