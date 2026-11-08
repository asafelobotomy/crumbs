use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::env::Env;
use crate::error::CoreError;

/// Directories external programs may be resolved from, joined to `Env::root`.
pub const TRUSTED_DIRS: [&str; 4] = ["/usr/bin", "/usr/sbin", "/bin", "/sbin"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CmdOutput {
    pub status: i32,
    pub stdout: String,
    pub stderr: String,
}

pub trait CommandRunner: Send + Sync {
    fn run(&self, program: &Path, args: &[&str]) -> Result<CmdOutput, CoreError>;
}

/// Runs real processes with `LC_ALL=C` and a null stdin.
pub struct SystemRunner;

impl CommandRunner for SystemRunner {
    fn run(&self, program: &Path, args: &[&str]) -> Result<CmdOutput, CoreError> {
        let out = Command::new(program)
            .args(args)
            .env("LC_ALL", "C")
            .stdin(Stdio::null())
            .output()
            .map_err(|e| CoreError::Command {
                program: program.display().to_string(),
                message: e.to_string(),
            })?;
        Ok(CmdOutput {
            status: out.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        })
    }
}

/// Finds `name` in the trusted directories only (never `$PATH`). Returns the
/// root-joined path, which is the path to execute.
pub fn resolve_trusted(env: &Env, name: &str) -> Option<PathBuf> {
    if name.is_empty() || name.contains('/') {
        return None;
    }
    TRUSTED_DIRS.iter().find_map(|dir| {
        let candidate = env.sys(dir).join(name);
        let meta = std::fs::metadata(&candidate).ok()?;
        (meta.is_file() && meta.permissions().mode() & 0o111 != 0).then_some(candidate)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::CoreError;
    use crate::testing::{FakeRunner, FixtureTree};

    #[test]
    fn resolves_from_usr_bin() {
        let t = FixtureTree::new();
        t.write_exec("/usr/bin/pacman", b"#!/bin/sh\n");
        let env = t.env();
        assert_eq!(
            resolve_trusted(&env, "pacman"),
            Some(env.sys("/usr/bin/pacman"))
        );
    }

    #[test]
    fn ignores_non_executable() {
        let t = FixtureTree::new();
        t.write("/usr/bin/pacman", b"#!/bin/sh\n");
        assert_eq!(resolve_trusted(&t.env(), "pacman"), None);
    }

    #[test]
    fn ignores_home_local_bin() {
        let t = FixtureTree::new();
        t.write_exec("/home/user/.local/bin/flatpak", b"#!/bin/sh\n");
        assert_eq!(resolve_trusted(&t.env(), "flatpak"), None);
    }

    #[test]
    fn rejects_name_with_slash() {
        let t = FixtureTree::new();
        t.write_exec("/usr/bin/pacman", b"#!/bin/sh\n");
        assert_eq!(resolve_trusted(&t.env(), "../usr/bin/pacman"), None);
        assert_eq!(resolve_trusted(&t.env(), "/usr/bin/pacman"), None);
    }

    #[test]
    fn fake_runner_returns_registered_output() {
        let out = CmdOutput {
            status: 0,
            stdout: "ok".into(),
            stderr: String::new(),
        };
        let r = FakeRunner::new().on("pacman", &["-Q"], out);
        let runner: &dyn CommandRunner = &r;
        let got = runner
            .run(Path::new("/some/root/usr/bin/pacman"), &["-Q"])
            .unwrap();
        assert_eq!(got.stdout, "ok");
        assert_eq!(r.calls(), ["pacman -Q"]);
    }

    #[test]
    fn fake_runner_errors_on_unregistered() {
        let r = FakeRunner::new();
        let err = r.run(Path::new("/usr/bin/pacman"), &["-Q"]).unwrap_err();
        assert!(matches!(err, CoreError::Command { .. }));
        assert_eq!(r.calls(), ["pacman -Q"]);
    }
}
