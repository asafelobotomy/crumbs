#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::os::unix::fs::MetadataExt;
use std::time::SystemTime;

use super::super::*;
use super::{parse_size, scale_1024};
use crate::command::CmdOutput;
use crate::config::journal_size_bytes;
use crate::testing::{FakeRunner, FixtureTree};

const PACCACHE_DRY: &str = "==> finished dry run: 1 candidates (disk space saved: 86.17 MiB)\n";
const JOURNAL: &str = "Archived and active journals take up 719.1M in the file system.\n";
const FLATPAK_LIST: [&str; 4] = ["list", "--system", "--runtime", "--columns=ref"];

fn out(status: i32, stdout: &str, stderr: &str) -> CmdOutput {
    CmdOutput {
        status,
        stdout: stdout.into(),
        stderr: stderr.into(),
        timed_out: false,
    }
}

fn strings(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

fn scan_cfg(t: &FixtureTree, runner: &FakeRunner, cfg: &Config, cat: Category) -> CategoryReport {
    let env = t.env();
    let inv = Inventory {
        apps: vec![],
        statuses: vec![],
        errors: vec![],
        owned: Default::default(),
    };
    let cx = ScanContext {
        env: &env,
        cfg,
        inv: &inv,
        runner,
        now: SystemTime::now(),
    };
    scan_all(&cx, &[cat]).categories.remove(0)
}

fn scan(t: &FixtureTree, runner: &FakeRunner, cat: Category) -> CategoryReport {
    scan_cfg(t, runner, &Config::default(), cat)
}

fn priv_action(program: &str, args: &[&str]) -> ItemAction {
    ItemAction::Privileged {
        program: program.into(),
        args: strings(args),
    }
}

fn orphan_fixture() -> (FixtureTree, FakeRunner) {
    let t = FixtureTree::new();
    t.write_exec("usr/bin/pacman", b"#!/bin/sh\n");
    t.pacman_pkg("foo", "1.0-1", Some(1000), &[]);
    t.pacman_pkg("bar", "2.0-1", Some(2000), &[]);
    t.pacman_pkg("libbaz", "3-1", Some(3000), &[]);
    t.pacman_pkg("keeper", "9-1", Some(9_000_000), &[]);
    let runner = FakeRunner::new().on("pacman", &["-Qdtq"], out(0, "foo\nbar\n", ""));
    (t, runner)
}

const SIM: [&str; 6] = ["-Rs", "--print", "--print-format", "%n %v", "foo", "bar"];

const APT_AUTOREMOVE: &str = include_str!("../../../../tests/fixtures/apt/autoremove.txt");
const DNF5_AUTOREMOVE: &str = include_str!("../../../../tests/fixtures/dnf/dnf5-autoremove.txt");

/// Runs the root step's script against a fake `timeshift` that lists
/// `listing` and logs every delete; returns the deleted names.
fn run_prune(listing: &str, keep: &str) -> (bool, Vec<String>) {
    run_prune_in(listing, keep, None)
}

/// [`run_prune`] with the root shell's working directory holding `file`.
fn run_prune_in(listing: &str, keep: &str, file: Option<&str>) -> (bool, Vec<String>) {
    let t = FixtureTree::new();
    let cwd = t.dir.path().join("cwd");
    std::fs::create_dir_all(&cwd).unwrap();
    if let Some(f) = file {
        std::fs::write(cwd.join(f), "").unwrap();
    }
    let bin = t.dir.path().join("bin");
    let log = t.dir.path().join("deleted");
    let fake = format!(
        "#!/bin/sh\ncase \"$1\" in\n--list) printf '%s' '{listing}' ;;\n--delete) echo \"$3\" >> '{}' ;;\nesac\n",
        log.display()
    );
    std::fs::create_dir_all(&bin).unwrap();
    let tool = bin.join("timeshift");
    std::fs::write(&tool, fake).unwrap();
    std::fs::set_permissions(&tool, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    let status = std::process::Command::new("/bin/sh")
        .current_dir(&cwd)
        .args([
            "-c",
            super::snapshots::PRUNE_TIMESHIFT,
            "crumbs-snapshots",
            keep,
        ])
        .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
        .status()
        .unwrap();
    let deleted = std::fs::read_to_string(&log).unwrap_or_default();
    (
        status.success(),
        deleted.lines().map(String::from).collect(),
    )
}

const LISTING: &str = "Mounted /dev/x at /run/timeshift/1/backup\nDevice : /dev/x\n\nNum     Name                 Tags  Description\n------------------------------------------------\n0    >  2026-11-08_14-00-06  D\n1    >  2026-10-24_12-00-00  O     {before upgrade}\n2    >  2026-11-10_08-18-24  O\n3    >  2026-11-09_13-11-59  D\n";

mod prune;
mod size_pkg;
mod system_flatpak;
