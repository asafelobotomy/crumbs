//! Fixture helpers for tests. Not part of the production API.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::HashMap;
use std::ffi::OsStr;
use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;

use crate::command::{CmdOutput, CommandRunner};
use crate::env::Env;
use crate::error::CoreError;
use crate::privilege::batch::{PrivilegedRun, PrivilegedRunner, StdioMode, stdio_modes};
use crate::privilege::{Escalation, Interaction};

pub struct FixtureTree {
    pub dir: tempfile::TempDir,
}

impl Default for FixtureTree {
    fn default() -> Self {
        Self::new()
    }
}

impl FixtureTree {
    pub fn new() -> Self {
        FixtureTree {
            dir: tempfile::tempdir().expect("create temp dir"),
        }
    }

    /// Builds an `Env` rooted at this tree; re-reads os-release each call.
    pub fn env(&self) -> Env {
        Env::fake(self.dir.path()).expect("fake env")
    }

    fn real(&self, rel: &str) -> PathBuf {
        self.dir.path().join(rel.trim_start_matches('/'))
    }

    /// Gives the fixture's `/tmp`, once it is a real directory, the sticky
    /// bit (mode 1777) like the real one; crumbs refuses a `/tmp` without
    /// it. Called by every helper that creates paths.
    fn sticky_tmp(&self) {
        let tmp = self.real("tmp");
        if let Ok(m) = fs::symlink_metadata(&tmp)
            && m.is_dir()
            && m.permissions().mode() & 0o1000 == 0
        {
            fs::set_permissions(&tmp, fs::Permissions::from_mode(0o1777)).unwrap();
        }
    }

    pub fn write(&self, rel: &str, contents: &[u8]) -> PathBuf {
        let p = self.real(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        self.sticky_tmp();
        fs::write(&p, contents).unwrap();
        p
    }

    /// Like `write`, but marks the file executable (mode 0755).
    pub fn write_exec(&self, rel: &str, contents: &[u8]) -> PathBuf {
        let p = self.write(rel, contents);
        fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).unwrap();
        p
    }

    pub fn symlink(&self, rel: &str, target: &str) {
        let p = self.real(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(target, &p).unwrap();
        self.sticky_tmp();
    }

    pub fn mkdir(&self, rel: &str) -> PathBuf {
        let p = self.real(rel);
        fs::create_dir_all(&p).unwrap();
        self.sticky_tmp();
        p
    }

    pub fn os_release(&self, id: &str, id_like: &str, name: &str) {
        let body = format!("NAME=\"{name}\"\nID={id}\nID_LIKE={id_like}\n");
        self.write("/etc/os-release", body.as_bytes());
    }

    /// Writes a pacman local-db entry (`desc` and `files`) for a package.
    pub fn pacman_pkg(&self, name: &str, version: &str, size: Option<u64>, files: &[&str]) {
        let dir = format!("/var/lib/pacman/local/{name}-{version}");
        let mut desc = format!("%NAME%\n{name}\n\n%VERSION%\n{version}\n\n");
        if let Some(s) = size {
            desc.push_str(&format!("%SIZE%\n{s}\n\n"));
        }
        self.write(&format!("{dir}/desc"), desc.as_bytes());
        let mut list = String::from("%FILES%\n");
        for f in files {
            list.push_str(f);
            list.push('\n');
        }
        self.write(&format!("{dir}/files"), list.as_bytes());
    }

    pub fn desktop(&self, rel_path: &str, contents: &str) {
        self.write(rel_path, contents.as_bytes());
    }

    pub fn appimage(&self, rel_path: &str) {
        let mut b = vec![0x7f, b'E', b'L', b'F', 0, 0, 0, 0, b'A', b'I', 0x02];
        b.resize(256, 0);
        self.write(rel_path, &b);
    }
}

/// A `CommandRunner` that returns canned output keyed by program basename
/// and exact args, and records every call as `"basename arg1 arg2"`.
#[derive(Default)]
pub struct FakeRunner {
    responses: HashMap<String, CmdOutput>,
    calls: Mutex<Vec<String>>,
}

impl FakeRunner {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn on(mut self, program_basename: &str, args: &[&str], out: CmdOutput) -> Self {
        self.responses
            .insert(Self::key(program_basename, args), out);
        self
    }

    pub fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }

    fn key(basename: &str, args: &[&str]) -> String {
        std::iter::once(basename)
            .chain(args.iter().copied())
            .collect::<Vec<_>>()
            .join(" ")
    }
}

impl CommandRunner for FakeRunner {
    fn run(&self, program: &Path, args: &[&str]) -> Result<CmdOutput, CoreError> {
        let base = program
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let key = Self::key(&base, args);
        self.calls.lock().unwrap().push(key.clone());
        self.responses
            .get(&key)
            .cloned()
            .ok_or_else(|| CoreError::Command {
                program: base,
                message: format!("no fake output registered for `{key}`"),
            })
    }
}

/// A `PrivilegedRunner` that runs the script with `/bin/sh -c` as the test
/// user (never escalating) and records every script it was given, and the
/// [`Interaction`] it ran with. Stdin is always `/dev/null`; stderr is
/// captured under `NoTerminal` and dropped otherwise.
#[derive(Default)]
pub struct FakePrivilegedRunner {
    pub calls: Mutex<Vec<Vec<u8>>>,
    pub interactions: Mutex<Vec<Interaction>>,
    interaction: Interaction,
}

impl FakePrivilegedRunner {
    pub fn with_interaction(i: Interaction) -> Self {
        FakePrivilegedRunner {
            interaction: i,
            ..Default::default()
        }
    }
}

impl PrivilegedRunner for FakePrivilegedRunner {
    fn run(&self, esc: &Escalation, script: &[u8]) -> Result<Vec<u8>, CoreError> {
        self.run_captured(esc, script).map(|r| r.stdout)
    }

    fn interaction(&self) -> Interaction {
        self.interaction
    }

    fn run_captured(&self, _esc: &Escalation, script: &[u8]) -> Result<PrivilegedRun, CoreError> {
        self.calls.lock().unwrap().push(script.to_vec());
        self.interactions.lock().unwrap().push(self.interaction);
        let (_, stderr) = stdio_modes(self.interaction);
        let out = Command::new("/bin/sh")
            .arg("-c")
            .arg(OsStr::from_bytes(script))
            .stdin(Stdio::null())
            .stderr(if stderr == StdioMode::Piped {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .output()
            .map_err(|e| CoreError::Command {
                program: "/bin/sh".into(),
                message: e.to_string(),
            })?;
        Ok(PrivilegedRun {
            stdout: out.stdout,
            stderr: out.stderr,
        })
    }
}
