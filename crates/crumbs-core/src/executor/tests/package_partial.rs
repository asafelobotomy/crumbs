use super::*;

#[test]
fn package_remove_post_check_still_installed() {
    let t = FixtureTree::new();
    let log = t.dir.path().join("calls.log");
    logging_program(&t, "pacman", &log, "", 0);
    let env = t.env();
    let plan = plan_of(vec![
        pkg_remove(&["foo", "bar"], &["1.0-1", "2.0-1"]),
        pkg_remove(&["baz"], &["3.0-1"]),
    ]);
    let seen = std::sync::Mutex::new(Vec::new());
    let post_check = |_: &Env, source: SourceKind, names: &[String]| {
        seen.lock().unwrap().push((source, names.to_vec()));
        Ok(names.iter().filter(|n| *n == "foo").cloned().collect())
    };
    let report = execute_full(
        &env,
        &Config::default(),
        &plan,
        &FakeRunner::new(),
        &FakePrivilegedRunner::default(),
        Some(&esc()),
        &post_check,
    );
    assert!(!report.results[0].ok);
    assert_eq!(
        report.results[0].error.as_deref(),
        Some("removed: bar; still installed: foo")
    );
    assert!(report.results[1].ok, "{:?}", report.results[1]);
    assert_eq!(report.freed, 100);
    assert_eq!(
        *seen.lock().unwrap(),
        [
            (SourceKind::Pacman, vec!["foo".to_string(), "bar".into()]),
            (SourceKind::Pacman, vec!["baz".to_string()]),
        ]
    );
    let (hist, _) = read_history(&env).unwrap();
    assert!(!hist[0].ok);
    assert_eq!(
        hist[0].error.as_deref(),
        Some("removed: bar; still installed: foo")
    );
    assert_eq!(
        hist[0].packages.as_deref(),
        Some(&[("bar".to_string(), "2.0-1".to_string())][..])
    );
}

#[test]
fn partial_markers_give_mixed_results() {
    let t = FixtureTree::new();
    let log = t.dir.path().join("calls.log");
    logging_program(&t, "flatpak", &log, "", 0);
    let env = t.env();
    let plan = plan_of(vec![
        priv_cmd("flatpak", &["a"]),
        priv_cmd("flatpak", &["b"]),
        priv_cmd("flatpak", &["c"]),
    ]);
    let report = execute_full(
        &env,
        &Config::default(),
        &plan,
        &FakeRunner::new(),
        &CannedRunner(b"|done\n@@crumbs 0 0\n|half\n".to_vec()),
        Some(&esc()),
        &no_post_check,
    );
    assert!(report.results[0].ok, "{:?}", report.results[0]);
    assert!(!report.results[1].ok);
    assert_eq!(report.results[1].error.as_deref(), Some(STOPPED_DURING));
    assert_eq!(report.results[2].error.as_deref(), Some(STOPPED_BEFORE));
    // The shell ran as root and was stopped: rights were not missing.
    assert!(!report.escalation_failed);
    assert!(report.batch_stopped);
    assert!(report.results.iter().all(|r| !r.rights_missing));
    // The history records what ran, and what did not.
    let (hist, _) = read_history(&env).unwrap();
    assert_eq!(hist.len(), 3);
    let oks: Vec<bool> = hist.iter().map(|h| h.ok).collect();
    assert_eq!(oks, [true, false, false]);
}

#[test]
fn stopped_package_removal_still_records_what_went() {
    let t = FixtureTree::new();
    let log = t.dir.path().join("calls.log");
    logging_program(&t, "flatpak", &log, "", 0);
    logging_program(&t, "pacman", &log, "", 0);
    let env = t.env();
    let plan = plan_of(vec![
        priv_cmd("flatpak", &["a"]),
        pkg_remove(&["foo", "bar"], &["1.0-1", "2.0-1"]),
    ]);
    // `bar` is gone, `foo` is still there: the step stopped part-way.
    let post = |_: &Env, _: SourceKind, _: &[String]| Ok(vec!["foo".to_string()]);
    let report = execute_full(
        &env,
        &Config::default(),
        &plan,
        &FakeRunner::new(),
        &CannedRunner(b"@@crumbs 0 0\n".to_vec()),
        Some(&esc()),
        &post,
    );
    let err = report.results[1].error.as_deref().unwrap();
    assert!(err.starts_with(STOPPED_DURING), "{err}");
    assert!(err.contains("removed: bar"), "{err}");
    assert!(err.contains("still installed: foo"), "{err}");
    assert!(!report.escalation_failed);
    let (hist, _) = read_history(&env).unwrap();
    let text = serde_json::to_string(&hist[1]).unwrap();
    assert!(
        text.contains("\"packages\":[[\"bar\",\"2.0-1\"]]"),
        "{text}"
    );
}

#[test]
fn too_large_batch_fails_without_escalation_failure() {
    let t = FixtureTree::new();
    let log = t.dir.path().join("calls.log");
    logging_program(&t, "flatpak", &log, "", 0);
    let env = t.env();
    let big = "x".repeat(130 * 1024);
    let plan = plan_of(vec![
        priv_cmd("flatpak", &["a"]),
        priv_cmd("flatpak", &[&big]),
    ]);
    let pr = DeniedRunner::default();
    let report = execute_full(
        &env,
        &Config::default(),
        &plan,
        &FakeRunner::new(),
        &pr,
        Some(&esc()),
        &no_post_check,
    );
    assert_eq!(*pr.calls.lock().unwrap(), 0);
    assert!(!report.escalation_failed);
    for r in &report.results {
        assert!(!r.ok);
        assert!(
            r.error.as_deref().unwrap().starts_with("too many items"),
            "{r:?}"
        );
    }
}

#[test]
fn dependent_of_package_failing_post_check_is_skipped() {
    let t = FixtureTree::new();
    let log = t.dir.path().join("calls.log");
    logging_program(&t, "pacman", &log, "", 0);
    t.write("home/user/.config/foo/a", b"a");
    let env = t.env();
    let mut leftover = planned(&env, trash("/home/user/.config/foo"));
    leftover.depends_on = Some(0);
    let plan = plan_of(vec![pkg_remove(&["foo"], &["1.0-1"]), leftover]);
    let report = execute_full(
        &env,
        &Config::default(),
        &plan,
        &FakeRunner::new(),
        &FakePrivilegedRunner::default(),
        Some(&esc()),
        &|_, _, names| Ok(names.to_vec()),
    );
    assert_eq!(
        report.results[0].error.as_deref(),
        Some("still installed: foo")
    );
    assert!(!report.results[1].ok);
    assert!(
        report.results[1]
            .error
            .as_deref()
            .unwrap()
            .starts_with("skipped: ")
    );
    assert!(env.sys("/home/user/.config/foo/a").exists());
}

#[test]
fn failed_package_step_reports_what_was_removed() {
    let t = FixtureTree::new();
    let log = t.dir.path().join("calls.log");
    logging_program(
        &t,
        "pacman",
        &log,
        "error: failed to commit transaction\n",
        1,
    );
    let env = t.env();
    let plan = plan_of(vec![pkg_remove(&["a", "b", "c"], &["1", "2", "3"])]);
    let still = |_: &Env, _: SourceKind, _: &[String]| Ok(vec!["c".to_string()]);
    let report = execute_full(
        &env,
        &Config::default(),
        &plan,
        &FakeRunner::new(),
        &FakePrivilegedRunner::default(),
        Some(&esc()),
        &still,
    );
    assert!(!report.results[0].ok);
    assert_eq!(
        report.results[0].error.as_deref(),
        Some(
            "pacman exited with status 1: error: failed to commit transaction; \
             removed: a, b; still installed: c"
        )
    );
    let (hist, _) = read_history(&env).unwrap();
    assert_eq!(
        hist[0].packages.as_deref(),
        Some(&[("a".to_string(), "1".to_string()), ("b".into(), "2".into())][..])
    );

    // A missing marker: the post-check still says what is gone.
    let t = FixtureTree::new();
    logging_program(&t, "pacman", &log, "", 0);
    let env = t.env();
    let none_left = |_: &Env, _: SourceKind, _: &[String]| Ok(Vec::new());
    let report = execute_full(
        &env,
        &Config::default(),
        &plan_of(vec![pkg_remove(&["a"], &["1"])]),
        &FakeRunner::new(),
        &DeniedRunner::default(),
        Some(&esc()),
        &none_left,
    );
    assert!(!report.results[0].ok);
    assert!(report.escalation_failed);
    assert_eq!(
        report.results[0].error.as_deref(),
        Some("privilege escalation was cancelled or denied; removed: a")
    );
}

#[test]
fn failed_system_tree_delete_restore_says_system_tree() {
    let (t, env) = system_tree_fixture();
    t.write("var/lib/pacman/db.lck", b"");
    let plan = plan_of(vec![tree_delete(&env, "/opt/inky")]);
    let (report, _) = run_tree(&env, &plan);
    assert!(!report.results[0].ok);
    let id = report.results[0].history_id.clone();
    let out = crate::restore::restore(&env, &Config::default(), &[id], false).unwrap();
    assert_eq!(out.len(), 1);
    assert!(!out[0].ok);
    assert_eq!(
        out[0].error.as_deref(),
        Some(crate::restore::MSG_SYSTEM_TREE)
    );
}

/// Without a real post-check (`execute`/`execute_with` until removal is
/// wired) a failed package step claims nothing about what was removed.
#[test]
fn default_post_check_claims_nothing_on_failure() {
    let t = FixtureTree::new();
    let log = t.dir.path().join("calls.log");
    logging_program(&t, "pacman", &log, "error: boom\n", 1);
    let env = t.env();
    let plan = plan_of(vec![pkg_remove(&["a", "b"], &["1", "2"])]);
    let run = |pr: &dyn PrivilegedRunner| {
        run_one(
            &env,
            &Config::default(),
            &plan,
            &FakeRunner::new(),
            pr,
            Some(&esc()),
            None,
        )
    };
    let report = run(&FakePrivilegedRunner::default());
    assert_eq!(
        report.results[0].error.as_deref(),
        Some("pacman exited with status 1: error: boom")
    );
    let report = run(&DeniedRunner::default());
    assert_eq!(report.results[0].error.as_deref(), Some(ESCALATION_DENIED));
    let (hist, _) = read_history(&env).unwrap();
    assert!(hist.iter().all(|h| h.packages.is_none()), "{hist:?}");

    // A step that succeeded is trusted: every package is recorded.
    logging_program(&t, "pacman", &log, "", 0);
    let report = run(&FakePrivilegedRunner::default());
    assert!(report.results[0].ok, "{:?}", report.results[0]);
    let (hist, _) = read_history(&env).unwrap();
    assert_eq!(
        hist.last().unwrap().packages.as_deref(),
        Some(&[("a".to_string(), "1".to_string()), ("b".into(), "2".into())][..])
    );
}
