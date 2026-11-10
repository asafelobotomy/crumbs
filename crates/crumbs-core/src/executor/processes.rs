//! Finding processes that belong to an app or use a path.

use super::*;

/// The kernel cuts `comm` at 15 bytes.
pub(super) const COMM_LEN: usize = 15;

/// A running process that runs a program from `in_system` (its executable
/// is there) or has a file open there, as `(pid, command name)`; the
/// lowest pid when there are several.
pub fn process_using(env: &Env, in_system: &Path) -> Option<(u32, String)> {
    let real = env.sys_path(in_system);
    // /proc shows resolved paths: a linked parent (`/home` -> `/data/home`)
    // would otherwise hide the process.
    let canon = fs::canonicalize(&real).ok();
    let inside = |p: &Path| {
        p.starts_with(in_system)
            || p.starts_with(&real)
            || canon.as_ref().is_some_and(|c| p.starts_with(c))
    };
    let rd = fs::read_dir(env.sys_path(Path::new("/proc"))).ok()?;
    rd.flatten()
        .filter_map(|e| {
            let pid: u32 = e.file_name().to_str()?.parse().ok()?;
            let dir = e.path();
            let exe = fs::read_link(dir.join("exe")).is_ok_and(|t| inside(&strip_deleted(&t)));
            let open = exe
                || fs::read_dir(dir.join("fd")).is_ok_and(|fds| {
                    fds.flatten()
                        .any(|f| fs::read_link(f.path()).is_ok_and(|t| inside(&t)))
                });
            if !open {
                return None;
            }
            let comm = fs::read_to_string(dir.join("comm")).unwrap_or_default();
            let comm = comm.trim();
            let comm = if comm.is_empty() { "a process" } else { comm };
            Some((pid, comm.to_string()))
        })
        .min()
}

/// A running process whose current directory is `in_system` or below it
/// (a build or a shell working there), as `(pid, command name)`; the
/// lowest pid when there are several.
pub fn process_working_in(env: &Env, in_system: &Path) -> Option<(u32, String)> {
    let real = env.sys_path(in_system);
    // /proc shows resolved paths: a linked parent (`/home` -> `/data/home`)
    // would otherwise hide the process.
    let canon = fs::canonicalize(&real).ok();
    let rd = fs::read_dir(env.sys_path(Path::new("/proc"))).ok()?;
    rd.flatten()
        .filter_map(|e| {
            let pid: u32 = e.file_name().to_str()?.parse().ok()?;
            let cwd = fs::read_link(e.path().join("cwd")).ok()?;
            if !(cwd.starts_with(in_system)
                || cwd.starts_with(&real)
                || canon.as_ref().is_some_and(|c| cwd.starts_with(c)))
            {
                return None;
            }
            let comm = fs::read_to_string(e.path().join("comm")).unwrap_or_default();
            let comm = comm.trim();
            let comm = if comm.is_empty() { "a process" } else { comm };
            Some((pid, comm.to_string()))
        })
        .min()
}

/// PIDs (sorted) of processes that look like `app`: the basename of the
/// `exe` link (without the kernel's ` (deleted)` suffix), of `comm` or of
/// `cmdline`'s first argument is one of `exec_names`; `cmdline`'s first
/// or second argument is the app's `program` (or `program_link`) path; the `exe` link is the
/// root-joined `location`; or `environ` holds `APPIMAGE=<location>`. Names
/// in [`GENERIC_BASENAMES`] never match by basename. Unreadable entries
/// are skipped.
pub fn running_processes(env: &Env, app: &App) -> Vec<u32> {
    let Ok(dir) = fs::read_dir(env.sys_path(Path::new("/proc"))) else {
        return Vec::new();
    };
    let location_real = app.location.as_deref().map(|l| env.sys_path(l));
    let appimage_var = app.location.as_deref().map(|l| {
        let mut v = b"APPIMAGE=".to_vec();
        v.extend_from_slice(l.as_os_str().as_bytes());
        v
    });
    let exec_names: Vec<&OsStr> = app
        .exec_names
        .iter()
        .filter_map(|e| Path::new(e).file_name())
        .filter(|n| !is_generic(n))
        .collect();
    let programs: Vec<&Path> = [app.program.as_deref(), app.program_link.as_deref()]
        .into_iter()
        .flatten()
        .collect();
    let named = |n: &OsStr| exec_names.contains(&n);
    let comm_named = |comm: &[u8]| {
        exec_names.iter().any(|n| {
            let n = n.as_bytes();
            n == comm || (n.len() > COMM_LEN && n[..COMM_LEN] == *comm)
        })
    };
    let mut pids: Vec<u32> = dir
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name();
            let name = name.to_str()?;
            if name.is_empty() || !name.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            let pid: u32 = name.parse().ok()?;
            let dir = entry.path();
            let by_exe = fs::read_link(dir.join("exe")).is_ok_and(|target| {
                let target = strip_deleted(&target);
                target.file_name().is_some_and(named)
                    || location_real.as_deref() == Some(target.as_path())
            });
            let by_comm = || {
                fs::read(dir.join("comm")).is_ok_and(|c| {
                    let c = c.strip_suffix(b"\n").unwrap_or(&c);
                    !c.is_empty() && comm_named(c)
                })
            };
            let by_argv0 = || {
                fs::read(dir.join("cmdline")).is_ok_and(|c| {
                    let argv0 = c.split(|&b| b == 0).next().unwrap_or_default();
                    Path::new(OsStr::from_bytes(argv0))
                        .file_name()
                        .is_some_and(named)
                })
            };
            let by_script = || {
                fs::read(dir.join("cmdline")).is_ok_and(|c| {
                    // A launcher script or interpreter names the program as
                    // argv0 or argv1; only its exact path counts, never the
                    // interpreter's name.
                    c.split(|&b| b == 0)
                        .take(2)
                        .any(|arg| programs.iter().any(|p| p.as_os_str().as_bytes() == arg))
                })
            };
            let by_env = || {
                appimage_var.as_deref().is_some_and(|var| {
                    fs::read(dir.join("environ"))
                        .is_ok_and(|bytes| bytes.split(|&b| b == 0).any(|kv| kv == var))
                })
            };
            (by_exe || by_comm() || by_argv0() || by_script() || by_env()).then_some(pid)
        })
        .collect();
    pids.sort_unstable();
    pids
}

/// `target` without the ` (deleted)` the kernel appends to the `exe` link of
/// a binary that was unlinked or replaced.
pub(super) fn strip_deleted(target: &Path) -> PathBuf {
    target
        .as_os_str()
        .as_bytes()
        .strip_suffix(b" (deleted)")
        .map_or_else(
            || target.to_path_buf(),
            |t| PathBuf::from(OsStr::from_bytes(t)),
        )
}
