use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use super::*;
use crate::command::CmdOutput;
use crate::history::{HistoryAction, read_history};
use crate::model::SourceKind;
use crate::plan::PlannedAction;
use crate::safety::{Fingerprint, Scope, fingerprint};
use crate::size::disk_usage;
use crate::testing::FakeRunner;
use crate::testing::FixtureTree;

/// Like `build_plan`: edits free nothing, removals free the disk usage.
fn planned(env: &Env, action: Action) -> PlannedAction {
    let p = action.path().unwrap().to_path_buf();
    let size = match action {
        Action::EditFile { .. } => 0,
        _ => disk_usage(env, &p).bytes,
    };
    PlannedAction {
        group: None,
        size,
        fingerprint: fingerprint(env, &p).unwrap(),
        scope: Scope::Home,
        depends_on: None,
        depends_on_plan: None,
        size_known: true,
        reason: "test".into(),
        action,
    }
}

fn plan_of(actions: Vec<PlannedAction>) -> Plan {
    Plan {
        app_id: Some("pacman:foo".into()),
        kind: Some("uninstall".into()),
        total_size: actions.iter().map(|a| a.size).sum(),
        actions,
        notes: Vec::new(),
    }
}

fn trash(p: &str) -> Action {
    Action::Trash { path: p.into() }
}

fn delete(p: &str) -> Action {
    Action::Delete { path: p.into() }
}

const MIME: &[u8] = b"[Default Applications]\r\ntext/plain=foo.desktop\r\nimage/png=other.desktop\n\n[Added Associations]\nx-scheme-handler/f=foo.desktop;\ntext/plain=foo.desktop;bar.desktop\n";

fn mime_edit() -> Action {
    Action::EditFile {
        path: "/home/user/.config/mimeapps.list".into(),
        remove_lines: vec![
            "text/plain=foo.desktop".into(),
            "x-scheme-handler/f=foo.desktop;".into(),
            String::new(),
        ],
    }
}

fn appimage_app() -> App {
    let mut app = App::new(SourceKind::AppImage, "Foo.AppImage");
    app.location = Some("/home/user/Apps/Foo.AppImage".into());
    app.exec_names = vec!["foo-bin".into()];
    app
}

/// Days the clock of [`temp_env`] runs ahead: an entry made now already
/// has an old ctime (which cannot be set), so its mtime decides its age.
const SHIFT_DAYS: u64 = 100;

/// The fixture's env with its clock [`SHIFT_DAYS`] ahead.
fn temp_env(t: &FixtureTree) -> crate::env::Env {
    let mut env = t.env();
    env.clock_shift = std::time::Duration::from_secs(SHIFT_DAYS * 86_400);
    env
}

/// Sets atime and mtime of `p` to `days` before the [`temp_env`] clock,
/// without following a symlink.
fn age_days(p: &Path, days: u64) {
    use rustix::fs::{AtFlags, CWD, Timespec, Timestamps, utimensat};
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
        + SHIFT_DAYS * 86_400
        - days * 86_400;
    let ts = Timespec {
        tv_sec: secs as i64,
        tv_nsec: 0,
    };
    let times = Timestamps {
        last_access: ts,
        last_modification: ts,
    };
    utimensat(CWD, p, &times, AtFlags::SYMLINK_NOFOLLOW).unwrap();
}

fn env_dir(t: &FixtureTree, rel: &str) -> PathBuf {
    t.dir.path().join(rel)
}

fn command_plan(program: &str, args: &[&str]) -> Plan {
    plan_of(vec![PlannedAction {
        group: None,
        action: Action::UserCommand {
            program: program.into(),
            args: args.iter().map(|a| (*a).to_string()).collect(),
        },
        size: 0,
        fingerprint: Fingerprint {
            dev: 0,
            ino: 0,
            is_symlink: false,
            ctime: 0,
            ctime_nsec: 0,
        },
        scope: Scope::Home,
        depends_on: None,
        depends_on_plan: None,
        size_known: true,
        reason: "test".into(),
    }])
}

// ---- Privileged batch (M4 Task 4) ----

use crate::privilege::batch::PrivilegedRunner;
use crate::privilege::{Escalation, EscalationKind};
use crate::testing::FakePrivilegedRunner;

fn esc() -> Escalation {
    Escalation {
        kind: EscalationKind::Pkexec,
        program: PathBuf::from("/usr/bin/pkexec"),
        requires_tty: false,
    }
}

fn no_post_check(_: &Env, _: SourceKind, _: &[String]) -> Result<Vec<String>, String> {
    Ok(Vec::new())
}

/// Steps in a script: one `__c <i>` or `__t<n> <i>` line per step.
fn step_count(script: &[u8]) -> usize {
    String::from_utf8_lossy(script)
        .lines()
        .filter(|l| {
            let word = l.split(' ').next().unwrap_or_default();
            word == "__c"
                || word
                    .strip_prefix("__t")
                    .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
        })
        .count()
}

fn unfingerprinted(action: Action) -> PlannedAction {
    PlannedAction {
        group: None,
        action,
        size: 0,
        fingerprint: Fingerprint {
            dev: 0,
            ino: 0,
            is_symlink: false,
            ctime: 0,
            ctime_nsec: 0,
        },
        scope: Scope::Home,
        depends_on: None,
        depends_on_plan: None,
        size_known: true,
        reason: "test".into(),
    }
}

fn priv_cmd(program: &str, args: &[&str]) -> PlannedAction {
    unfingerprinted(Action::PrivilegedCommand {
        program: program.into(),
        args: args.iter().map(|a| (*a).to_string()).collect(),
    })
}

fn pkg_remove(names: &[&str], versions: &[&str]) -> PlannedAction {
    let mut args = vec!["-Rns".to_string(), "--noconfirm".into()];
    args.extend(names.iter().map(|n| (*n).to_string()));
    let mut pa = unfingerprinted(Action::PackageRemove {
        source: SourceKind::Pacman,
        names: names.iter().map(|n| (*n).to_string()).collect(),
        versions: versions.iter().map(|v| (*v).to_string()).collect(),
        program: "pacman".into(),
        args,
    });
    pa.size = 100;
    pa
}

/// A fixture `/var/lib/systemd/coredump` whose chain passes the System
/// scope checks (see `Env::system_owner_uid`).
fn coredump_dir(t: &FixtureTree) {
    t.mkdir("var/lib/systemd/coredump");
    let mut cur = t.dir.path().to_path_buf();
    fs::set_permissions(&cur, fs::Permissions::from_mode(0o755)).unwrap();
    for c in ["var", "lib", "systemd", "coredump"] {
        cur.push(c);
        fs::set_permissions(&cur, fs::Permissions::from_mode(0o755)).unwrap();
    }
}

fn core_delete(env: &Env, name: &str) -> PlannedAction {
    let p = PathBuf::from(format!("/var/lib/systemd/coredump/{name}"));
    PlannedAction {
        group: None,
        size: disk_usage(env, &p).bytes,
        fingerprint: fingerprint(env, &p).unwrap(),
        action: Action::Delete { path: p },
        scope: Scope::System,
        depends_on: None,
        depends_on_plan: None,
        size_known: true,
        reason: "test".into(),
    }
}

/// A fixture `rm` that runs the real one (on the root-joined path the
/// executor passes).
fn fixture_rm(t: &FixtureTree) {
    t.write_exec("usr/bin/rm", b"#!/bin/sh\nexec /usr/bin/rm \"$@\"\n");
}

/// A fixture program that appends its arguments to `log` and exits with
/// `status`, printing `output` first.
fn logging_program(t: &FixtureTree, name: &str, log: &Path, output: &str, status: i32) {
    let script = format!(
        "#!/bin/sh\necho \"$@\" >> '{}'\nprintf '%s' '{output}'\nexit {status}\n",
        log.display()
    );
    t.write_exec(&format!("usr/bin/{name}"), script.as_bytes());
}

/// A runner whose escalation produced no output at all (cancelled or
/// denied): it records the script but runs nothing.
#[derive(Default)]
struct DeniedRunner {
    calls: std::sync::Mutex<usize>,
}

impl PrivilegedRunner for DeniedRunner {
    fn run(&self, _: &Escalation, _: &[u8]) -> Result<Vec<u8>, crate::error::CoreError> {
        *self.calls.lock().unwrap() += 1;
        Ok(Vec::new())
    }
}

/// Denies like [`DeniedRunner`], with what the escalation wrote on stderr.
struct DeniedWithStderr;

impl PrivilegedRunner for DeniedWithStderr {
    fn run(&self, _: &Escalation, _: &[u8]) -> Result<Vec<u8>, crate::error::CoreError> {
        Ok(Vec::new())
    }

    fn run_captured(
        &self,
        _: &Escalation,
        _: &[u8],
    ) -> Result<crate::privilege::batch::PrivilegedRun, crate::error::CoreError> {
        Ok(crate::privilege::batch::PrivilegedRun {
            stdout: Vec::new(),
            stderr: b"polkit: not authorized\n".to_vec(),
        })
    }
}

/// A runner that hands back canned stdout without running anything.
struct CannedRunner(Vec<u8>);

impl PrivilegedRunner for CannedRunner {
    fn run(&self, _: &Escalation, _: &[u8]) -> Result<Vec<u8>, crate::error::CoreError> {
        Ok(self.0.clone())
    }
}

fn app_plan(app_id: &str, actions: Vec<PlannedAction>) -> Plan {
    let mut p = plan_of(actions);
    p.app_id = Some(app_id.into());
    p
}

/// A mountinfo line for `mount` (a root-joined path in fixtures).
fn mountinfo_line(mount: &Path) -> String {
    format!(
        "36 35 98:0 /mnt1 {} rw,noatime master:1 - ext3 /dev/root rw\n",
        mount.display()
    )
}

/// A fake root where `/opt/inky` and `/opt/blot` pass every SystemTree
/// check: a 0755 chain owned by the test user, a pacman database that owns
/// only `/opt/foo`, a mountinfo with nothing below `/opt`, and fixture `sh`,
/// `readlink` and `rm` that run the real ones.
fn system_tree_fixture() -> (FixtureTree, Env) {
    let t = FixtureTree::new();
    fixture_rm(&t);
    t.write_exec("usr/bin/sh", b"#!/bin/sh\nexec /bin/sh \"$@\"\n");
    t.write_exec(
        "usr/bin/readlink",
        b"#!/bin/sh\nexec /usr/bin/readlink \"$@\"\n",
    );
    t.pacman_pkg("foo", "1-1", Some(1), &["opt/foo/", "opt/foo/x"]);
    t.write("opt/inky/a", b"a");
    t.write("opt/inky/sub/b", b"b");
    t.write("opt/blot/c", b"c");
    t.write(
        "proc/self/mountinfo",
        mountinfo_line(Path::new("/")).as_bytes(),
    );
    for rel in ["", "opt"] {
        fs::set_permissions(t.dir.path().join(rel), fs::Permissions::from_mode(0o755)).unwrap();
    }
    let mut env = t.env();
    env.set_system_owner_uid(env.uid);
    (t, env)
}

fn tree_delete(env: &Env, p: &str) -> PlannedAction {
    let mut pa = planned(env, delete(p));
    pa.scope = Scope::SystemTree;
    pa
}

fn run_tree(env: &Env, plan: &Plan) -> (ExecReport, Vec<Vec<u8>>) {
    let pr = FakePrivilegedRunner::default();
    let report = execute_full(
        env,
        &Config::default(),
        plan,
        &FakeRunner::new(),
        &pr,
        Some(&esc()),
        &no_post_check,
    );
    let calls = pr.calls.lock().unwrap().clone();
    (report, calls)
}

/// The user's mount namespace, as the guard compares it; `None` (with a
/// message, so the caller skips) when `/proc` cannot be read here.
fn own_mount_ns() -> Option<std::ffi::OsString> {
    match fs::read_link("/proc/self/ns/mnt") {
        Ok(ns) => Some(ns.into_os_string()),
        Err(e) => {
            eprintln!("skipped: /proc/self/ns/mnt is unreadable here ({e})");
            None
        }
    }
}

/// The guard script a tree step in `env` runs, from the real builder.
fn guard_script(env: &Env) -> std::ffi::OsString {
    super::tree_script(
        &env.sys("/var/lib/pacman/db.lck"),
        &env.sys("/usr/bin/readlink"),
        &env.sys("/usr/bin/rm"),
    )
}

/// A finding for an existing in-system path, as the scan makes it.
fn scanned(
    env: &Env,
    path: &str,
    kind: crate::leftovers::FindingKind,
    scope: Scope,
) -> crate::leftovers::Finding {
    let p = PathBuf::from(path);
    crate::leftovers::Finding {
        size: disk_usage(env, &p),
        fingerprint: fingerprint(env, &p).unwrap(),
        path: p,
        kind,
        confidence: crate::leftovers::Confidence::Medium,
        shared: false,
        reason: "test".into(),
        preselected: scope == Scope::Home,
        scope,
        mime_lines: Vec::new(),
    }
}

/// A privileged runner that removes `remove` (real paths) as the batch
/// would, then hands back canned markers.
struct RemovingRunner {
    remove: Vec<PathBuf>,
    out: Vec<u8>,
}

impl PrivilegedRunner for RemovingRunner {
    fn run(&self, _: &Escalation, _: &[u8]) -> Result<Vec<u8>, crate::error::CoreError> {
        for p in &self.remove {
            let _ = std::fs::remove_file(p);
        }
        Ok(self.out.clone())
    }
}

/// A command runner that removes `remove` and succeeds.
struct RemovingCommand(Vec<PathBuf>);

impl CommandRunner for RemovingCommand {
    fn run(
        &self,
        _: &Path,
        _: &[&str],
    ) -> Result<crate::command::CmdOutput, crate::error::CoreError> {
        for p in &self.0 {
            let _ = std::fs::remove_file(p);
        }
        Ok(crate::command::CmdOutput {
            status: 0,
            stdout: String::new(),
            stderr: String::new(),
            timed_out: false,
        })
    }
}

mod denied_no;
mod execute;
mod execute_edit;
mod measurement_commands;
mod package_partial;
mod running_generic;
mod system_empty;
mod unverifiable_cross;
mod user_temp;
