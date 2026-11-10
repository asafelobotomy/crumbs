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

/// `git -C <repo> check-ignore -- <rels>` as the scanner asks it.
fn ignore_args(t: &FixtureTree, repo: &str, rels: &[&str]) -> Vec<String> {
    let mut v = vec![
        "-C".to_string(),
        t.dir
            .path()
            .join(repo.trim_start_matches('/'))
            .display()
            .to_string(),
        "check-ignore".into(),
        "--".into(),
    ];
    v.extend(rels.iter().map(|r| (*r).to_string()));
    v
}

fn on_git(runner: FakeRunner, args: &[String], ignored: &[&str]) -> FakeRunner {
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let stdout: String = ignored.iter().map(|r| format!("{r}\n")).collect();
    runner.on(
        "git",
        &args,
        out(if ignored.is_empty() { 1 } else { 0 }, &stdout),
    )
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
    // Look-alikes: no project file beside them, or not ignored.
    t.write(&format!("{r}/docs/build/index.html"), b"<html>");
    t.write(&format!("{r}/kept/package.json"), b"{}");
    t.write(&format!("{r}/kept/node_modules/y"), b"y");
    t
}

fn rels() -> [&'static str; 5] {
    [
        "android/app/build",
        "kept/node_modules",
        "target",
        "tools/.venv",
        "web/node_modules",
    ]
}

#[test]
fn lists_ignored_build_output_beside_its_project_file() {
    let t = projects();
    let args = ignore_args(&t, "/home/user/Projects/app", &rels());
    let runner = on_git(
        FakeRunner::new(),
        &args,
        &[
            "android/app/build",
            "target",
            "tools/.venv",
            "web/node_modules",
        ],
    );
    let cfg = Config::default();
    let items = scan_cfg(&t, &runner, &cfg);
    let mut got: Vec<_> = items.iter().map(|i| i.label.clone()).collect();
    got.sort();
    assert_eq!(
        got,
        [
            "app/android/app/build",
            "app/target",
            "app/tools/.venv",
            "app/web/node_modules"
        ]
    );
    let env = t.env();
    for i in &items {
        assert_eq!(i.category, Category::BuildOutput);
        assert_eq!(i.scope, Scope::Home);
        assert_eq!(i.action, ItemAction::Trash);
        assert!(i.rebuildable && i.preselected, "{i:?}");
        assert!(
            i.note.as_deref().unwrap().starts_with("last used "),
            "{i:?}"
        );
        verify(
            &env,
            &cfg,
            Scope::Home,
            i.path.as_ref().unwrap(),
            i.fingerprint.as_ref().unwrap(),
        )
        .unwrap();
    }
}

#[test]
fn recent_or_busy_build_output_is_not_preselected() {
    let t = projects();
    let target = t.dir.path().join("home/user/Projects/app/target");
    touch(&target, 1);
    let args = ignore_args(&t, "/home/user/Projects/app", &rels());
    let runner = on_git(FakeRunner::new(), &args, &["target", "web/node_modules"]);
    let items = scan_cfg(&t, &runner, &Config::default());
    let item = |l: &str| items.iter().find(|i| i.label == l).unwrap();
    assert!(!item("app/target").preselected);
    assert_eq!(
        item("app/target").note.as_deref(),
        Some("last used 1 day ago")
    );
    assert!(item("app/web/node_modules").preselected);

    // A process working anywhere in the repository holds all of it back.
    t.symlink("proc/88/cwd", "/home/user/Projects/app/web");
    t.write("proc/88/comm", b"npm\n");
    let runner = on_git(FakeRunner::new(), &args, &["target", "web/node_modules"]);
    let items = scan_cfg(&t, &runner, &Config::default());
    for i in &items {
        assert!(!i.preselected);
        assert_eq!(i.note.as_deref(), Some("app is in use by npm (pid 88)"));
    }
}

#[test]
fn no_git_or_git_error_lists_nothing() {
    let t = projects();
    // git says it is not a repository (exit 128): nothing is trusted.
    let args = ignore_args(&t, "/home/user/Projects/app", &rels());
    let a: Vec<&str> = args.iter().map(String::as_str).collect();
    let runner = FakeRunner::new().on("git", &a, out(128, ""));
    assert!(scan_cfg(&t, &runner, &Config::default()).is_empty());
    // No trusted git at all.
    let t = FixtureTree::new();
    t.write("home/user/Projects/app/.git/HEAD", b"ref");
    t.write("home/user/Projects/app/Cargo.toml", b"");
    t.write("home/user/Projects/app/target/x", b"x");
    assert!(scan_cfg(&t, &FakeRunner::new(), &Config::default()).is_empty());
}

#[test]
fn project_dirs_from_config() {
    let t = FixtureTree::new();
    t.write_exec("usr/bin/git", b"#!/bin/sh\n");
    t.mkdir("home/user/work/deep/lib/.git");
    t.write("home/user/work/deep/lib/Cargo.toml", b"");
    t.write("home/user/work/deep/lib/target/x", b"x");
    t.mkdir("home/user/Projects/other/.git");
    t.write("home/user/Projects/other/Cargo.toml", b"");
    t.write("home/user/Projects/other/target/x", b"x");
    let cfg = Config {
        project_dirs: vec![PathBuf::from("/home/user/work")],
        ..Config::default()
    };
    let args = ignore_args(&t, "/home/user/work/deep/lib", &["target"]);
    let runner = on_git(FakeRunner::new(), &args, &["target"]);
    let items = scan_cfg(&t, &runner, &cfg);
    let labels: Vec<_> = items.iter().map(|i| i.label.clone()).collect();
    assert_eq!(labels, ["deep/lib/target"]);
}
