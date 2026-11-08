//! Detection of the privilege-escalation program to use for root operations.

use std::path::PathBuf;

use serde::Serialize;

use crate::command::{CommandRunner, resolve_trusted};
use crate::env::Env;

/// Oldest systemd release whose `run0` is trusted for escalation.
const MIN_RUN0_SYSTEMD: u32 = 256;

#[derive(Serialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum EscalationKind {
    Run0,
    Pkexec,
    Sudo,
}

#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
pub struct Escalation {
    pub kind: EscalationKind,
    /// In-system path of the escalation program.
    pub program: PathBuf,
    pub requires_tty: bool,
}

/// Parses the systemd major version from the first line of
/// `systemctl --version` (`systemd 261 (261.3-1-manjaro)`).
fn systemd_version(stdout: &str) -> Option<u32> {
    let line = stdout.lines().next()?;
    let mut words = line.split_whitespace();
    (words.next()? == "systemd").then_some(())?;
    words.next()?.parse().ok()
}

fn modern_systemd(env: &Env, runner: &dyn CommandRunner) -> bool {
    let Some(systemctl) = resolve_trusted(env, "systemctl") else {
        return false;
    };
    match runner.run(&systemctl, &["--version"]) {
        Ok(out) if out.status == 0 => {
            systemd_version(&out.stdout).is_some_and(|v| v >= MIN_RUN0_SYSTEMD)
        }
        _ => false,
    }
}

/// Picks run0 (systemd 256+), else pkexec, else sudo (needs a TTY).
pub fn detect_escalation(env: &Env, runner: &dyn CommandRunner) -> Option<Escalation> {
    let found = |kind, path: PathBuf, requires_tty| Escalation {
        kind,
        program: env.unsys(&path),
        requires_tty,
    };
    if let Some(run0) = resolve_trusted(env, "run0")
        && modern_systemd(env, runner)
    {
        return Some(found(EscalationKind::Run0, run0, false));
    }
    if let Some(p) = resolve_trusted(env, "pkexec") {
        return Some(found(EscalationKind::Pkexec, p, false));
    }
    resolve_trusted(env, "sudo").map(|p| found(EscalationKind::Sudo, p, true))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::CmdOutput;
    use crate::testing::{FakeRunner, FixtureTree};

    fn version(stdout: &str) -> FakeRunner {
        FakeRunner::new().on(
            "systemctl",
            &["--version"],
            CmdOutput {
                status: 0,
                stdout: stdout.into(),
                stderr: String::new(),
                timed_out: false,
            },
        )
    }

    #[test]
    fn run0_requires_systemd_256() {
        let t = FixtureTree::new();
        for p in ["run0", "pkexec", "systemctl"] {
            t.write_exec(&format!("/usr/bin/{p}"), b"");
        }
        let runner = version("systemd 255 (255.4-1)\n+PAM\n");
        let e = detect_escalation(&t.env(), &runner).unwrap();
        assert_eq!(e.kind, EscalationKind::Pkexec);
        assert_eq!(e.program, PathBuf::from("/usr/bin/pkexec"));
        assert!(!e.requires_tty);
    }

    #[test]
    fn run0_selected_on_261() {
        let t = FixtureTree::new();
        for p in ["run0", "pkexec", "systemctl"] {
            t.write_exec(&format!("/usr/bin/{p}"), b"");
        }
        let runner = version("systemd 261 (261.3-1-manjaro)\n");
        let e = detect_escalation(&t.env(), &runner).unwrap();
        assert_eq!(e.kind, EscalationKind::Run0);
        assert_eq!(e.program, PathBuf::from("/usr/bin/run0"));
        assert!(!e.requires_tty);
        assert_eq!(runner.calls(), ["systemctl --version"]);
    }

    #[test]
    fn run0_skipped_when_version_unknown() {
        let t = FixtureTree::new();
        for p in ["run0", "pkexec", "systemctl"] {
            t.write_exec(&format!("/usr/bin/{p}"), b"");
        }
        let e = detect_escalation(&t.env(), &version("garbage\n")).unwrap();
        assert_eq!(e.kind, EscalationKind::Pkexec);
        // systemctl failing to run (no fake registered) also falls through.
        let e = detect_escalation(&t.env(), &FakeRunner::new()).unwrap();
        assert_eq!(e.kind, EscalationKind::Pkexec);
    }

    #[test]
    fn sudo_fallback_requires_tty() {
        let t = FixtureTree::new();
        t.write_exec("/usr/bin/sudo", b"");
        let e = detect_escalation(&t.env(), &FakeRunner::new()).unwrap();
        assert_eq!(e.kind, EscalationKind::Sudo);
        assert_eq!(e.program, PathBuf::from("/usr/bin/sudo"));
        assert!(e.requires_tty);
    }

    #[test]
    fn none_when_nothing_present() {
        let t = FixtureTree::new();
        assert_eq!(detect_escalation(&t.env(), &FakeRunner::new()), None);
    }

    #[test]
    fn kind_serializes_kebab_case() {
        assert_eq!(serde_json::to_value(EscalationKind::Run0).unwrap(), "run0");
    }
}
