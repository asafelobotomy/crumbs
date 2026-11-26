use super::*;

#[test]
fn execute_trashes_and_logs() {
    let t = FixtureTree::new();
    t.write("home/user/.config/foo/a", b"aaaa");
    t.write("home/user/.cache/foo/b", b"bbbb");
    let env = t.env();
    let plan = plan_of(vec![
        planned(&env, trash("/home/user/.config/foo")),
        planned(&env, trash("/home/user/.cache/foo")),
    ]);
    let report = execute(&env, &Config::default(), &plan);
    assert!(!report.batch.is_empty());
    assert_eq!(report.results.len(), 2);
    for (i, r) in report.results.iter().enumerate() {
        assert!(r.ok, "{r:?}");
        assert_eq!(r.error, None);
        assert_eq!(r.history_id, format!("{}-{i}", report.batch));
        assert_eq!(r.action, plan.actions[i].action);
    }
    assert_eq!(report.freed, plan.total_size);
    assert!(!env.sys("/home/user/.config/foo").exists());
    assert!(!env.sys("/home/user/.cache/foo").exists());

    let (hist, skipped) = read_history(&env).unwrap();
    assert_eq!(skipped, 0);
    assert_eq!(hist.len(), 2);
    for (i, h) in hist.iter().enumerate() {
        assert_eq!(h.id, report.results[i].history_id);
        assert_eq!(h.batch, report.batch);
        assert_eq!(h.action, HistoryAction::Trash);
        assert_eq!(Some(h.path.as_path()), plan.actions[i].action.path());
        assert!(h.ok);
        assert_eq!(h.app_id.as_deref(), Some("pacman:foo"));
        assert_eq!(h.size, plan.actions[i].size);
        let tr = h.trashed.as_ref().unwrap();
        assert_eq!(&tr.original, &h.path);
        assert!(env.sys_path(&tr.files_path).exists());
    }
}

/// Review Focus #1 end to end, for both trash and permanent delete.
#[test]
fn execute_refuses_swapped_path() {
    for permanent in [false, true] {
        let t = FixtureTree::new();
        t.write("home/user/.config/foo/a", b"a");
        t.write("home/user/Documents/precious", b"keep me");
        let env = t.env();
        let target = "/home/user/.config/foo";
        let action = if permanent {
            delete(target)
        } else {
            trash(target)
        };
        let plan = plan_of(vec![planned(&env, action)]);
        fs::remove_dir_all(env.sys(target)).unwrap();
        let docs = t.dir.path().join("home/user/Documents");
        std::os::unix::fs::symlink(&docs, env.sys(target)).unwrap();

        let report = execute(&env, &Config::default(), &plan);
        let r = &report.results[0];
        assert!(!r.ok);
        assert!(r.error.as_deref().unwrap().contains("changed"), "{r:?}");
        assert_eq!(report.freed, 0);
        assert_eq!(fs::read(docs.join("precious")).unwrap(), b"keep me");
        assert!(
            fs::symlink_metadata(env.sys(target))
                .unwrap()
                .file_type()
                .is_symlink()
        );
        let (hist, _) = read_history(&env).unwrap();
        assert_eq!(hist.len(), 1);
        assert!(!hist[0].ok);
        assert!(hist[0].error.is_some());
        assert_eq!(hist[0].trashed, None);
    }
}

#[test]
fn execute_partial_failure_continues() {
    let t = FixtureTree::new();
    for d in ["a", "b", "c"] {
        t.write(&format!("home/user/.config/{d}/f"), b"data");
    }
    let env = t.env();
    let plan = plan_of(vec![
        planned(&env, trash("/home/user/.config/a")),
        planned(&env, trash("/home/user/.config/b")),
        planned(&env, delete("/home/user/.config/c")),
    ]);
    fs::remove_dir_all(env.sys("/home/user/.config/b")).unwrap();
    let report = execute(&env, &Config::default(), &plan);
    let oks: Vec<bool> = report.results.iter().map(|r| r.ok).collect();
    assert_eq!(oks, [true, false, true]);
    assert!(
        report.results[1]
            .error
            .as_deref()
            .unwrap()
            .contains("no longer exists")
    );
    assert_eq!(report.freed, plan.actions[0].size + plan.actions[2].size);
    assert!(!env.sys("/home/user/.config/c").exists());
    let (hist, _) = read_history(&env).unwrap();
    let logged: Vec<(HistoryAction, bool)> = hist.iter().map(|h| (h.action, h.ok)).collect();
    assert_eq!(
        logged,
        [
            (HistoryAction::Trash, true),
            (HistoryAction::Trash, false),
            (HistoryAction::Delete, true)
        ]
    );
}

#[test]
fn execute_delete_removes_link_not_target() {
    let t = FixtureTree::new();
    t.write("home/user/target/keep", b"x");
    t.write("home/user/filelink-target", b"y");
    let env = t.env();
    t.symlink("home/user/.config/dirlink", "../target");
    t.symlink("home/user/.config/filelink", "../filelink-target");
    let plan = plan_of(vec![
        planned(&env, delete("/home/user/.config/dirlink")),
        planned(&env, delete("/home/user/.config/filelink")),
    ]);
    let report = execute(&env, &Config::default(), &plan);
    assert!(report.results.iter().all(|r| r.ok), "{report:?}");
    assert!(fs::symlink_metadata(env.sys("/home/user/.config/dirlink")).is_err());
    assert!(fs::symlink_metadata(env.sys("/home/user/.config/filelink")).is_err());
    assert!(env.sys("/home/user/target/keep").exists());
    assert!(env.sys("/home/user/filelink-target").exists());
}

#[test]
fn execute_refuses_protected_target() {
    let t = FixtureTree::new();
    t.write("home/user/.ssh/id", b"k");
    let env = t.env();
    let plan = plan_of(vec![planned(&env, delete("/home/user/.ssh/id"))]);
    let report = execute(&env, &Config::default(), &plan);
    assert!(!report.results[0].ok);
    assert!(env.sys("/home/user/.ssh/id").exists());
}

/// A plan's scope reaches `verify`: a `/tmp` child is deleted under
/// `UserTemp` and refused under the default `Home`.
#[test]
fn execute_passes_scope_to_verify() {
    if crate::testing::running_as_root() {
        eprintln!("skipped: temp cleanup is not offered as root");
        return;
    }
    let t = FixtureTree::new();
    let a = t.write("tmp/a", b"a");
    age_days(&a, 30);
    t.write("tmp/b", b"b");
    let env = temp_env(&t);
    let mut temp = planned(&env, delete("/tmp/a"));
    temp.scope = Scope::UserTemp;
    let home = planned(&env, delete("/tmp/b"));
    assert_eq!(home.scope, Scope::Home);
    let report = execute(&env, &Config::default(), &plan_of(vec![temp, home]));
    assert!(report.results[0].ok, "{:?}", report.results[0]);
    assert!(!report.results[1].ok);
    assert_eq!(
        report.results[1].error.as_deref(),
        Some("refusing: /tmp/b is outside the allowed roots")
    );
    assert!(!env.sys("/tmp/a").exists());
    assert!(env.sys("/tmp/b").exists());
}

/// A `/tmp` dir that was stale when scanned but gained a fresh file
/// before execution is refused and left intact.
#[test]
fn execute_rechecks_temp_staleness() {
    if crate::testing::running_as_root() {
        eprintln!("skipped: temp cleanup is not offered as root");
        return;
    }
    let t = FixtureTree::new();
    let f = t.write("tmp/work/old", b"x");
    let dir = env_dir(&t, "tmp/work");
    age_days(&f, 30);
    age_days(&dir, 30);
    let g = t.write("tmp/gone/old", b"x");
    let gone = env_dir(&t, "tmp/gone");
    age_days(&g, 30);
    age_days(&gone, 30);
    let env = temp_env(&t);
    let mut stale = planned(&env, delete("/tmp/work"));
    stale.scope = Scope::UserTemp;
    let mut ok = planned(&env, delete("/tmp/gone"));
    ok.scope = Scope::UserTemp;
    // Someone starts using the dir between scan and clean; the dir's own
    // inode is unchanged, so `verify` alone would pass.
    age_days(&t.write("tmp/work/new", b"fresh"), 0);
    age_days(&dir, 30);
    let report = execute(&env, &Config::default(), &plan_of(vec![stale, ok]));
    assert!(!report.results[0].ok);
    assert_eq!(
        report.results[0].error.as_deref(),
        Some("refusing: /tmp/work is no longer an old temporary file")
    );
    assert!(env.sys("/tmp/work/old").exists());
    assert!(env.sys("/tmp/work/new").exists());
    assert!(report.results[1].ok, "{:?}", report.results[1]);
    assert!(!env.sys("/tmp/gone").exists());
}

#[test]
fn execute_with_on_fake_root_never_escalates() {
    let t = FixtureTree::new();
    let log = t.dir.path().join("calls.log");
    logging_program(&t, "pacman", &log, "", 0);
    // An escalation program detection would otherwise pick.
    let esc_log = t.dir.path().join("pkexec.log");
    logging_program(&t, "pkexec", &esc_log, "", 0);
    let env = t.env();
    let plan = plan_of(vec![pkg_remove(&["foo"], &["1.0-1"])]);
    let runner = FakeRunner::new();
    let report = execute_with(&env, &Config::default(), &plan, &runner);
    assert_eq!(report.results[0].error.as_deref(), Some(NO_ESCALATION));
    assert!(report.escalation_failed);
    assert!(!esc_log.exists(), "the escalation program ran");
    assert!(!log.exists());
    assert!(runner.calls().is_empty(), "{:?}", runner.calls());
}

#[test]
fn execute_many_uses_one_batch_for_every_plan() {
    let t = FixtureTree::new();
    let log = t.dir.path().join("calls.log");
    logging_program(&t, "pacman", &log, "", 0);
    t.write("home/user/.config/foo/a", b"a");
    t.write("home/user/.config/bar/a", b"a");
    let env = t.env();
    let mut foo_left = planned(&env, trash("/home/user/.config/foo"));
    foo_left.depends_on = Some(0);
    let mut bar_left = planned(&env, trash("/home/user/.config/bar"));
    bar_left.depends_on = Some(0);
    let plans = [
        app_plan("pacman:foo", vec![pkg_remove(&["foo"], &["1-1"]), foo_left]),
        app_plan("pacman:bar", vec![pkg_remove(&["bar"], &["2-1"]), bar_left]),
    ];
    let pr = FakePrivilegedRunner::default();
    let reports = execute_many(
        &env,
        &Config::default(),
        &plans,
        &FakeRunner::new(),
        &pr,
        Some(&esc()),
        &no_post_check,
    );
    assert_eq!(reports.len(), 2);
    let calls = pr.calls.lock().unwrap().clone();
    assert_eq!(calls.len(), 1, "one escalation for the whole run");
    assert_eq!(step_count(&calls[0]), 2);
    assert_eq!(
        fs::read_to_string(&log).unwrap(),
        "-Rns --noconfirm foo\n-Rns --noconfirm bar\n"
    );
    for r in &reports {
        assert!(r.results.iter().all(|x| x.ok), "{r:?}");
        assert!(!r.escalation_failed);
    }
    assert_ne!(reports[0].batch, reports[1].batch);
    assert!(!env.sys("/home/user/.config/bar").exists());
    // History keeps each plan's batch id and app.
    let (hist, _) = read_history(&env).unwrap();
    assert_eq!(hist.len(), 4);
    for h in &hist {
        let (batch, app) = if h.batch == reports[0].batch {
            (&reports[0].batch, "pacman:foo")
        } else {
            (&reports[1].batch, "pacman:bar")
        };
        assert_eq!(&h.batch, batch);
        assert_eq!(h.app_id.as_deref(), Some(app), "{h:?}");
    }
    // A later run gets a later batch id than both.
    let next: u128 = crate::history::next_batch_id(&env).parse().unwrap();
    assert!(next > reports[1].batch.parse::<u128>().unwrap());
}
