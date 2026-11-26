use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use super::*;
use crate::bloat::ItemAction;
use crate::command::CmdOutput;
use crate::config::Config;
use crate::inventory::Inventory;
use crate::safety::{Scope, verify};
use crate::testing::{FakeRunner, FixtureTree};

/// The scan's clock: 100 days ahead, so every fixture file is old.
fn now() -> SystemTime {
    SystemTime::now() + Duration::from_secs(100 * 86_400)
}

fn out(status: i32, stdout: &str) -> CmdOutput {
    CmdOutput {
        status,
        stdout: stdout.into(),
        stderr: String::new(),
        timed_out: false,
    }
}

fn scan_cfg(t: &FixtureTree, runner: &FakeRunner, cfg: &Config) -> Vec<BloatItem> {
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
        now: now(),
    };
    BuildOutputScanner.scan(&cx).unwrap()
}

/// `git -C <repo> ... ls-files --others --ignored ... -- <rels>` as the
/// scanner asks it.
fn ignore_args(t: &FixtureTree, repo: &str, rels: &[&str]) -> Vec<String> {
    let real = t
        .dir
        .path()
        .join(repo.trim_start_matches('/'))
        .display()
        .to_string();
    let mut v = vec![
        "-C".to_string(),
        real.clone(),
        "--literal-pathspecs".into(),
        format!("--work-tree={real}"),
        "-c".into(),
        "core.fsmonitor=false".into(),
        "-c".into(),
        "core.untrackedCache=false".into(),
        "ls-files".into(),
        "--others".into(),
        "--ignored".into(),
        "--exclude-standard".into(),
        "--directory".into(),
        "-z".into(),
        "--".into(),
    ];
    v.extend(rels.iter().map(|r| (*r).to_string()));
    v
}

/// The candidates in [`projects`]' repository, sorted.
const APP_RELS: [&str; 7] = [
    "android/app/build",
    "desktop/dist",
    "game/.godot",
    "kept/node_modules",
    "target",
    "tools/.venv",
    "web/node_modules",
];

/// git answers with these ignored folders (`/` appended) and files.
fn on_git(runner: FakeRunner, args: &[String], ignored: &[&str]) -> FakeRunner {
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let stdout: String = ignored
        .iter()
        .map(|r| {
            if r.ends_with(".txt") || r.ends_with('/') {
                format!("{r}\0")
            } else {
                format!("{r}/\0")
            }
        })
        .collect();
    runner.on("git", &args, out(0, &stdout))
}

fn touch(p: &Path, days_before_now: u64) {
    let when = now() - Duration::from_secs(days_before_now * 86_400);
    std::fs::File::open(p).unwrap().set_modified(when).unwrap();
}

/// A repository with one of each kind, all ignored by git but `kept/`.
fn projects() -> FixtureTree {
    let t = FixtureTree::new();
    t.write_exec("usr/bin/git", b"#!/bin/sh\n");
    let r = "home/user/Projects/app";
    t.mkdir(&format!("{r}/.git"));
    t.write(&format!("{r}/Cargo.toml"), b"[package]");
    t.write(&format!("{r}/target/debug/app"), &[1u8; 4000]);
    t.write(&format!("{r}/web/package.json"), b"{}");
    t.write(&format!("{r}/web/node_modules/x/index.js"), b"x");
    t.write(&format!("{r}/android/app/build.gradle.kts"), b"");
    t.write(&format!("{r}/android/app/build/out.apk"), b"apk");
    t.write(&format!("{r}/tools/.venv/pyvenv.cfg"), b"home = /usr/bin");
    t.write(&format!("{r}/desktop/package.json"), b"{}");
    t.write(&format!("{r}/desktop/electron-builder.yml"), b"appId: x");
    t.write(&format!("{r}/desktop/dist/app.AppImage"), b"elf");
    t.write(&format!("{r}/game/project.godot"), b"[application]");
    t.write(&format!("{r}/game/.godot/imported/a.ctex"), b"tex");
    // Look-alikes: no project file beside them, or not ignored.
    t.write(&format!("{r}/docs/build/index.html"), b"<html>");
    t.write(&format!("{r}/site/package.json"), b"{}");
    t.write(&format!("{r}/site/dist/index.html"), b"<html>");
    t.write(&format!("{r}/kept/package.json"), b"{}");
    t.write(&format!("{r}/kept/node_modules/y"), b"y");
    t
}

/// Runs the real git in `dir` without the caller's `GIT_*` variables (a
/// git hook running the tests could point them at another repository).
fn git_cmd(git: &Path, dir: &Path, args: &[&str]) {
    let mut cmd = std::process::Command::new(git);
    cmd.args(args).current_dir(dir);
    for (k, _) in std::env::vars_os() {
        if k.to_string_lossy().starts_with("GIT_") {
            cmd.env_remove(k);
        }
    }
    let ok = cmd.status().unwrap();
    assert!(ok.success(), "{args:?}");
}

mod a_virtual;
mod lists_recent;
