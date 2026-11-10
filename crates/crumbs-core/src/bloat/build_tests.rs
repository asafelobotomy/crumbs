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
        "-c".into(),
        "core.fsmonitor=false".into(),
        "-c".into(),
        "core.untrackedCache=false".into(),
        "-c".into(),
        "core.quotePath=false".into(),
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

#[test]
fn nested_repositories_answer_for_themselves() {
    // vendor/inner is its own repository inside an ignored folder of the
    // outer one: its folders are judged by its own git, not the outer's.
    let t = FixtureTree::new();
    t.write_exec("usr/bin/git", b"#!/bin/sh\n");
    let outer = "home/user/Projects/outer";
    t.mkdir(&format!("{outer}/.git"));
    t.write(&format!("{outer}/Cargo.toml"), b"");
    t.write(&format!("{outer}/target/x"), b"x");
    let inner = format!("{outer}/vendor/inner");
    t.write(
        &format!("{inner}/.git"),
        b"gitdir: ../../.git/modules/inner",
    );
    t.write(&format!("{inner}/CMakeLists.txt"), b"");
    t.write(&format!("{inner}/build/keep"), b"tracked");
    let runner = on_git(
        FakeRunner::new(),
        &ignore_args(&t, "/home/user/Projects/outer", &["target"]),
        &["target"],
    );
    // The inner repository tracks its build folder: git ignores nothing.
    let runner = on_git(
        runner,
        &ignore_args(&t, "/home/user/Projects/outer/vendor/inner", &["build"]),
        &[],
    );
    let items = scan_cfg(&t, &runner, &Config::default());
    let labels: Vec<_> = items.iter().map(|i| i.label.clone()).collect();
    assert_eq!(labels, ["outer/target"]);
}

#[test]
fn non_ascii_paths_are_read_unquoted() {
    // git quotes non-ASCII paths unless told not to; they would never match.
    let t = FixtureTree::new();
    t.write_exec("usr/bin/git", b"#!/bin/sh\n");
    let r = "home/user/Projects/app";
    t.mkdir(&format!("{r}/.git"));
    t.write(&format!("{r}/\u{fc}n\u{ef}/build.gradle"), b"");
    t.write(&format!("{r}/\u{fc}n\u{ef}/build/x"), b"x");
    let rel = "\u{fc}n\u{ef}/build";
    let runner = on_git(
        FakeRunner::new(),
        &ignore_args(&t, "/home/user/Projects/app", &[rel]),
        &[rel],
    );
    let items = scan_cfg(&t, &runner, &Config::default());
    assert_eq!(items.len(), 1);
}

#[test]
fn a_hostile_git_config_runs_nothing() {
    // A repository's config can name commands git runs (core.fsmonitor);
    // a scan must never run them. Uses the real git, when there is one.
    let Some(git) = ["/usr/bin/git", "/bin/git"]
        .into_iter()
        .map(Path::new)
        .find(|p| p.exists())
    else {
        eprintln!("skipped: no git");
        return;
    };
    let t = FixtureTree::new();
    let repo = t.dir.path().join("home/user/Projects/app");
    std::fs::create_dir_all(&repo).unwrap();
    let ok = std::process::Command::new(git)
        .args(["init", "-q"])
        .current_dir(&repo)
        .status()
        .unwrap();
    assert!(ok.success());
    let pwned = t.dir.path().join("PWNED");
    std::fs::write(
        repo.join(".git/config"),
        format!("[core]\n\tfsmonitor = touch {}\n", pwned.display()),
    )
    .unwrap();
    std::fs::write(repo.join(".gitignore"), "target/\n").unwrap();
    std::fs::write(repo.join("Cargo.toml"), "").unwrap();
    std::fs::create_dir_all(repo.join("target")).unwrap();
    let env = t.env();
    let cfg = Config::default();
    let inv = Inventory {
        apps: vec![],
        statuses: vec![],
        errors: vec![],
        owned: Default::default(),
    };
    let runner = crate::command::SystemRunner::new();
    let cx = ScanContext {
        env: &env,
        cfg: &cfg,
        inv: &inv,
        runner: &runner,
        now: now(),
    };
    let ignored = ignored_by_git(&cx, git, &repo, &["target".to_string()]);
    assert_eq!(ignored, ["target"]);
    assert!(
        !pwned.exists(),
        "git ran the repository's fsmonitor command"
    );
}
