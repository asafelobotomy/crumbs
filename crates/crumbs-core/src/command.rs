use std::ffi::OsString;
use std::io::Read;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

use rustix::process::{Pid, Signal, kill_process_group};
use std::thread;
use std::time::{Duration, Instant};

use crate::env::Env;
use crate::error::CoreError;

/// Directories external programs may be resolved from, joined to `Env::root`.
pub const TRUSTED_DIRS: [&str; 4] = ["/usr/bin", "/usr/sbin", "/bin", "/sbin"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CmdOutput {
    pub status: i32,
    pub stdout: String,
    pub stderr: String,
    /// The process was killed because it ran past its timeout; `status` is
    /// then `-1` and the output is whatever was read before the kill.
    pub timed_out: bool,
}

pub trait CommandRunner: Send + Sync {
    fn run(&self, program: &Path, args: &[&str]) -> Result<CmdOutput, CoreError>;
}

/// Variables a child may inherit; everything else is dropped.
pub(crate) const ENV_ALLOWLIST: [&str; 9] = [
    "HOME",
    "USER",
    "LOGNAME",
    "XDG_RUNTIME_DIR",
    "XDG_CONFIG_HOME",
    "XDG_DATA_HOME",
    "XDG_CACHE_HOME",
    "XDG_STATE_HOME",
    "DBUS_SESSION_BUS_ADDRESS",
];
pub(crate) const CHILD_PATH: &str = "/usr/bin:/usr/sbin:/bin:/sbin";
const QUERY_TIMEOUT: Duration = Duration::from_secs(60);
/// How long a `UserCommand` may run.
pub const USER_COMMAND_TIMEOUT: Duration = Duration::from_secs(30 * 60);
const POLL_INTERVAL: Duration = Duration::from_millis(20);

/// Runs real processes with a cleared environment (an allowlist plus a fixed
/// `PATH` and `LC_ALL=C`), a null stdin and a timeout.
pub struct SystemRunner {
    timeout: Duration,
}

impl SystemRunner {
    pub fn new() -> Self {
        Self {
            timeout: QUERY_TIMEOUT,
        }
    }

    pub fn with_timeout(timeout: Duration) -> Self {
        Self { timeout }
    }

    fn run_with_env(
        &self,
        program: &Path,
        args: &[&str],
        parent: &[(OsString, OsString)],
    ) -> Result<CmdOutput, CoreError> {
        let fail = |e: std::io::Error| CoreError::Command {
            program: program.display().to_string(),
            message: e.to_string(),
        };
        let mut cmd = Command::new(program);
        // Its own process group, so a timeout can kill grandchildren too.
        cmd.args(args).env_clear().process_group(0);
        for (k, v) in parent {
            if ENV_ALLOWLIST.iter().any(|a| k == *a) {
                cmd.env(k, v);
            }
        }
        cmd.env("PATH", CHILD_PATH).env("LC_ALL", "C");
        cmd.stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = spawn_when_not_busy(&mut cmd).map_err(fail)?;
        // Readers run beside the wait so a chatty child can't fill a pipe
        // and block forever.
        let out = child.stdout.take().map(read_all);
        let err = child.stderr.take().map(read_all);
        let deadline = Instant::now() + self.timeout;
        let (status, timed_out) = loop {
            match child.try_wait() {
                Ok(Some(st)) => break (st.code().unwrap_or(-1), false),
                Ok(None) if Instant::now() >= deadline => {
                    kill_group(&mut child);
                    break (-1, true);
                }
                Ok(None) => thread::sleep(POLL_INTERVAL),
                Err(e) => {
                    kill_group(&mut child);
                    return Err(fail(e));
                }
            }
        };
        let text = |h: Option<thread::JoinHandle<Vec<u8>>>| {
            h.and_then(|h| h.join().ok())
                .map(|b| String::from_utf8_lossy(&b).into_owned())
                .unwrap_or_default()
        };
        Ok(CmdOutput {
            status,
            stdout: text(out),
            stderr: text(err),
            timed_out,
        })
    }
}

impl Default for SystemRunner {
    fn default() -> Self {
        Self::new()
    }
}

/// Spawns `cmd`, retrying for a short while if exec reports ETXTBSY.
///
/// A script that was just written may still be open for writing in a process
/// another thread forked at that moment; exec refuses it until that fd closes
/// on the forked child's exec, which takes milliseconds.
fn spawn_when_not_busy(cmd: &mut Command) -> std::io::Result<Child> {
    let mut wait = Duration::from_millis(5);
    let deadline = Instant::now() + Duration::from_millis(500);
    loop {
        match cmd.spawn() {
            Err(e) if e.raw_os_error() == Some(rustix::io::Errno::TXTBSY.raw_os_error()) => {
                if Instant::now() >= deadline {
                    return Err(e);
                }
                thread::sleep(wait);
                wait = (wait * 2).min(Duration::from_millis(50));
            }
            other => return other,
        }
    }
}

/// SIGKILLs the child's whole process group (it leads one) and reaps the
/// child; descendants holding the output pipes die with it.
fn kill_group(child: &mut Child) {
    if let Some(pid) = i32::try_from(child.id()).ok().and_then(Pid::from_raw) {
        let _ = kill_process_group(pid, Signal::KILL);
    }
    let _ = child.kill();
    let _ = child.wait();
}

fn read_all<R: Read + Send + 'static>(mut r: R) -> thread::JoinHandle<Vec<u8>> {
    thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = r.read_to_end(&mut buf);
        buf
    })
}

impl CommandRunner for SystemRunner {
    fn run(&self, program: &Path, args: &[&str]) -> Result<CmdOutput, CoreError> {
        let parent: Vec<(OsString, OsString)> = std::env::vars_os().collect();
        self.run_with_env(program, args, &parent)
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
    use std::ffi::OsString;
    use std::time::Duration;

    fn os(s: &str) -> OsString {
        OsString::from(s)
    }

    #[test]
    fn system_runner_retries_while_text_file_busy() {
        // Another thread forking while a fixture script is still open for
        // writing makes exec fail with ETXTBSY for a moment; the runner must
        // wait it out rather than fail.
        let t = FixtureTree::new();
        let script = t.write_exec("/usr/bin/busy-fixture", b"#!/bin/sh\necho ok\n");
        let writer = std::fs::OpenOptions::new()
            .append(true)
            .open(&script)
            .unwrap();
        let release = thread::spawn(move || {
            thread::sleep(Duration::from_millis(60));
            drop(writer);
        });
        let out = SystemRunner::new().run(&script, &[]).unwrap();
        release.join().unwrap();
        assert_eq!(out.status, 0, "{out:?}");
        assert_eq!(out.stdout, "ok\n");
    }

    #[test]
    fn system_runner_gives_up_on_a_file_that_stays_busy() {
        let t = FixtureTree::new();
        let script = t.write_exec("/usr/bin/busy-fixture", b"#!/bin/sh\necho ok\n");
        let _writer = std::fs::OpenOptions::new()
            .append(true)
            .open(&script)
            .unwrap();
        let start = Instant::now();
        let err = SystemRunner::new().run(&script, &[]).unwrap_err();
        assert!(err.to_string().contains("busy"), "{err}");
        assert!(start.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn system_runner_clears_env() {
        let t = FixtureTree::new();
        let script = t.write_exec("/usr/bin/printenv-fixture", b"#!/bin/sh\n/usr/bin/env\n");
        let parent = [
            (os("LD_PRELOAD"), os("/tmp/evil.so")),
            (os("FOO"), os("bar")),
            (os("HOME"), os("/home/kept")),
            (os("PATH"), os("/tmp/evil")),
            (os("LC_ALL"), os("de_DE.UTF-8")),
        ];
        let out = SystemRunner::new()
            .run_with_env(&script, &[], &parent)
            .unwrap();
        assert_eq!(out.status, 0, "{out:?}");
        let lines: Vec<&str> = out.stdout.lines().collect();
        assert!(!lines.iter().any(|l| l.starts_with("LD_PRELOAD=")));
        assert!(!lines.iter().any(|l| l.starts_with("FOO=")));
        assert!(lines.contains(&"HOME=/home/kept"));
        assert!(lines.contains(&"PATH=/usr/bin:/usr/sbin:/bin:/sbin"));
        assert!(lines.contains(&"LC_ALL=C"));
        assert!(!out.timed_out);
    }

    #[test]
    fn system_runner_times_out() {
        let started = std::time::Instant::now();
        let out = SystemRunner::with_timeout(Duration::from_millis(200))
            .run(Path::new("/bin/sleep"), &["5"])
            .unwrap();
        assert!(out.timed_out);
        assert_eq!(out.status, -1);
        assert!(started.elapsed() < Duration::from_secs(4));
    }

    /// Whether a live (not zombie) process is in process group `pgid`.
    fn group_alive(pgid: i32) -> bool {
        let Ok(dir) = std::fs::read_dir("/proc") else {
            return false;
        };
        dir.flatten().any(|e| {
            let Ok(stat) = std::fs::read_to_string(e.path().join("stat")) else {
                return false;
            };
            // `<pid> (<comm>) <state> <ppid> <pgrp> …`; comm may hold spaces.
            let Some((_, rest)) = stat.rsplit_once(") ") else {
                return false;
            };
            let mut fields = rest.split(' ');
            let state = fields.next();
            let pgrp = fields.nth(1).and_then(|p| p.parse::<i32>().ok());
            pgrp == Some(pgid) && state != Some("Z")
        })
    }

    #[test]
    fn system_runner_timeout_kills_the_process_group() {
        let t = FixtureTree::new();
        let pidfile = t.dir.path().join("pgid");
        // The shell records its pid (its process group) first, then
        // backgrounds a sleep that inherits the pipes and waits on another;
        // killing only the shell would leave both running.
        let script = format!(
            "#!/bin/sh\necho $$ > {}\nsleep 10 &\nsleep 10\n",
            pidfile.display()
        );
        let prog = t.write_exec("/usr/bin/grandkids", script.as_bytes());
        let started = std::time::Instant::now();
        let out = SystemRunner::with_timeout(Duration::from_secs(1))
            .run(&prog, &[])
            .unwrap();
        assert!(out.timed_out);
        assert!(started.elapsed() < Duration::from_secs(4));
        let pgid: i32 = std::fs::read_to_string(&pidfile)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        // SIGKILL is asynchronous and the killed processes linger as
        // zombies until reaped: wait for every live member to go.
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while group_alive(pgid) {
            assert!(
                std::time::Instant::now() < deadline,
                "process group {pgid} survived"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn system_runner_drains_large_output() {
        let out = SystemRunner::new()
            .run(Path::new("/bin/sh"), &["-c", "head -c 300000 /dev/zero"])
            .unwrap();
        assert_eq!(out.status, 0);
        assert!(!out.timed_out);
        assert_eq!(out.stdout.len(), 300_000);
    }

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
            timed_out: false,
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
