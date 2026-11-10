//! Running a batch script with root rights through run0, pkexec or sudo.

use super::*;

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
pub(super) const TRUNCATED: &[u8] = "… (truncated)\n".as_bytes();

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
    pub(super) fn stdio(self) -> Stdio {
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
pub(super) fn escalation_refused(root: &Path, in_test: bool) -> Option<&'static str> {
    if in_test {
        Some("refusing to escalate inside tests")
    } else if root != Path::new("/") {
        Some("refusing to escalate under a fake root")
    } else {
        None
    }
}

/// The escalation program's arguments for running `script` in a root shell.
pub(super) fn escalation_args(esc: &Escalation, script: &[u8]) -> Vec<OsString> {
    let mut args = Vec::with_capacity(4);
    match esc.kind {
        EscalationKind::Run0 | EscalationKind::Sudo => args.push(OsString::from("--")),
        EscalationKind::Pkexec => {}
        // The program is the shell itself.
        EscalationKind::AlreadyRoot => {
            args.push(OsString::from("-c"));
            args.push(OsStr::from_bytes(script).to_os_string());
            return args;
        }
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
