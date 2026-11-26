use super::*;

#[test]
fn a_repository_cannot_point_git_at_another_work_tree() {
    // core.worktree in the repository's own config would make git judge
    // its folders by another folder's ignore rules.
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
    let elsewhere = t.dir.path().join("elsewhere");
    std::fs::create_dir_all(repo.join("target")).unwrap();
    std::fs::write(repo.join("target/work"), "").unwrap();
    std::fs::create_dir_all(elsewhere.join("target")).unwrap();
    std::fs::write(elsewhere.join(".gitignore"), "target/\n").unwrap();
    git_cmd(git, &repo, &["init", "-q"]);
    git_cmd(
        git,
        &repo,
        &["config", "core.worktree", &elsewhere.display().to_string()],
    );
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
    assert!(ignored_by_git(&cx, git, &repo, &["target".to_string()]).is_empty());
}

#[test]
fn a_linked_parent_of_home_keeps_items_under_home() {
    // /home is a link to /data/home: resolving ~/Projects must not move
    // items out of home as the safety checks see it.
    let t = FixtureTree::new();
    t.write_exec("usr/bin/git", b"#!/bin/sh\n");
    t.mkdir("data/home/user/Projects/app/.git");
    t.write("data/home/user/Projects/app/Cargo.toml", b"");
    t.write("data/home/user/Projects/app/target/x", b"x");
    t.symlink("home", "data/home");
    let args = ignore_args(&t, "/data/home/user/Projects/app", &["target"]);
    let runner = on_git(FakeRunner::new(), &args, &["target"]);
    let cfg = Config::default();
    let items = scan_cfg(&t, &runner, &cfg);
    assert_eq!(items.len(), 1, "{items:?}");
    let p = items[0].path.clone().unwrap();
    assert_eq!(p, PathBuf::from("/home/user/Projects/app/target"));
    verify(
        &t.env(),
        &cfg,
        Scope::Home,
        &p,
        items[0].fingerprint.as_ref().unwrap(),
    )
    .unwrap();
}

#[test]
fn a_process_seen_through_the_linked_parent_still_holds_it_back() {
    // /proc shows the resolved path of a process working in the repository.
    let t = FixtureTree::new();
    t.write_exec("usr/bin/git", b"#!/bin/sh\n");
    t.mkdir("data/home/user/Projects/app/.git");
    t.write("data/home/user/Projects/app/Cargo.toml", b"");
    t.write("data/home/user/Projects/app/target/x", b"x");
    t.symlink("home", "data/home");
    let real_repo = t.dir.path().join("data/home/user/Projects/app");
    t.symlink("proc/5/cwd", real_repo.to_str().unwrap());
    t.write("proc/5/comm", b"cargo\n");
    let args = ignore_args(&t, "/data/home/user/Projects/app", &["target"]);
    let runner = on_git(FakeRunner::new(), &args, &["target"]);
    let items = scan_cfg(&t, &runner, &Config::default());
    assert!(!items[0].preselected, "{:?}", items[0]);
    assert_eq!(
        items[0].note.as_deref(),
        Some("app is in use by cargo (pid 5)")
    );
}

#[test]
fn virtual_environments_are_never_preselected() {
    // Something outside the project (a service, a script) may run from a
    // venv, and it may have no requirements file to rebuild it from.
    let t = projects();
    let args = ignore_args(&t, "/home/user/Projects/app", &APP_RELS);
    let runner = on_git(FakeRunner::new(), &args, &["tools/.venv"]);
    let items = scan_cfg(&t, &runner, &Config::default());
    assert_eq!(items.len(), 1);
    let v = &items[0];
    assert!(!v.preselected && !v.rebuildable, "{v:?}");
    assert_eq!(
        v.note.as_deref(),
        Some(
            "a virtual environment can be in use from outside the project; last changed 100 days ago"
        )
    );
}

#[test]
fn real_git_reports_exact_paths() {
    // With the real git: a path with a quote comes back as it is, and a
    // build folder holding a tracked file is not reported.
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
    for d in ["q\"x/build", "part/build"] {
        std::fs::create_dir_all(repo.join(d)).unwrap();
        std::fs::write(repo.join(d).join("f"), "").unwrap();
    }
    // An untracked file beside the tracked one: git lists it, not the
    // folder, and that must not make the folder count.
    std::fs::write(repo.join("part/build/o"), "").unwrap();
    let git_in = |args: &[&str]| git_cmd(git, &repo, args);
    git_in(&["init", "-q"]);
    std::fs::write(repo.join(".gitignore"), "build/\n").unwrap();
    git_in(&["add", "-f", "part/build/f"]);
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
    let rels = ["part/build".to_string(), "q\"x/build".to_string()];
    assert_eq!(ignored_by_git(&cx, git, &repo, &rels), ["q\"x/build"]);
}
