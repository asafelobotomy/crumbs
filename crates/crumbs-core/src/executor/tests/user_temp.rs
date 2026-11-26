use super::*;

#[test]
fn user_cleanup_reports_measured_space() {
    let t = FixtureTree::new();
    logging_program(&t, "flatpak", &t.dir.path().join("l.log"), "", 0);
    let rt = t.write(
        "home/user/.local/share/flatpak/runtime/x/files/lib.so",
        &[1u8; 12_000],
    );
    t.write(
        "home/user/.local/share/flatpak/app/y/files/bin",
        &[1u8; 4_000],
    );
    let env = t.env();
    let gone = crate::size::disk_usage(
        &env,
        Path::new("/home/user/.local/share/flatpak/runtime/x/files/lib.so"),
    )
    .bytes;
    let plan = command_plan(
        "flatpak",
        &["uninstall", "--user", "--unused", "--noninteractive", "-y"],
    );
    let report = execute_with(&env, &Config::default(), &plan, &RemovingCommand(vec![rt]));
    assert!(report.results[0].ok, "{:?}", report.results[0]);
    assert_eq!(report.results[0].freed, Some(gone));
    assert_eq!(report.freed, gone);
}

#[test]
fn temp_delete_verified_in_user_temp_scope() {
    if crate::testing::running_as_root() {
        eprintln!("skipped: temp cleanup is not offered as root");
        return;
    }
    use crate::bloat::{BloatItem, Category, ItemAction};
    let t = FixtureTree::new();
    let a = t.write("tmp/a", b"a");
    let b = t.write("tmp/b", b"b");
    age_days(&a, 30);
    age_days(&b, 30);
    let env = temp_env(&t);
    let item = |path: &str| {
        let p = PathBuf::from(path);
        BloatItem {
            size_uncompressed: false,
            category: Category::Temp,
            size: Some(1),
            fingerprint: Some(fingerprint(&env, &p).unwrap()),
            path: Some(p),
            label: path.into(),
            rebuildable: false,
            preselected: true,
            note: None,
            action: ItemAction::Delete,
            scope: Scope::UserTemp,
            extra_paths: Vec::new(),
            app_id: None,
        }
    };
    let items = [item("/tmp/a"), item("/tmp/b")];
    let plan = crate::plan::build_clean_plan(&env, &items, &Default::default()).unwrap();
    // /tmp/b is swapped for a different inode after the scan.
    fs::remove_file(env.sys("/tmp/b")).unwrap();
    let nb = t.write("tmp/b", b"other");
    age_days(&nb, 30);
    let report = execute(&env, &Config::default(), &plan);
    assert!(report.results[0].ok, "{:?}", report.results[0]);
    assert!(!env.sys("/tmp/a").exists());
    assert!(!report.results[1].ok);
    assert!(env.sys("/tmp/b").exists());
    let (entries, _) = read_history(&env).unwrap();
    assert_eq!(entries[0].action, HistoryAction::Delete);
    assert_eq!(entries[0].path, Path::new("/tmp/a"));
}

#[test]
fn privileged_actions_run_in_one_batch() {
    let t = FixtureTree::new();
    let log = t.dir.path().join("calls.log");
    logging_program(&t, "flatpak", &log, "", 0);
    logging_program(&t, "pacman", &log, "removing foo...\n", 0);
    fixture_rm(&t);
    coredump_dir(&t);
    t.write("var/lib/systemd/coredump/core.x.1", b"dump");
    t.write("home/user/.config/foo/a", b"a");
    let env = t.env();
    let plan = plan_of(vec![
        planned(&env, trash("/home/user/.config/foo")),
        priv_cmd("flatpak", &["uninstall", "--system", "-y", "org.x.App"]),
        pkg_remove(&["foo"], &["1.0-1"]),
        core_delete(&env, "core.x.1"),
    ]);
    let pr = FakePrivilegedRunner::default();
    let report = execute_full(
        &env,
        &Config::default(),
        &plan,
        &FakeRunner::new(),
        &pr,
        Some(&esc()),
        &no_post_check,
    );
    for r in &report.results {
        assert!(r.ok, "{r:?}");
    }
    assert!(!report.escalation_failed);
    let calls = pr.calls.lock().unwrap().clone();
    assert_eq!(calls.len(), 1, "one escalation for the whole run");
    assert_eq!(step_count(&calls[0]), 3);
    assert_eq!(
        fs::read_to_string(&log).unwrap(),
        "uninstall --system -y org.x.App\n-Rns --noconfirm foo\n"
    );
    assert!(!env.sys("/var/lib/systemd/coredump/core.x.1").exists());
    assert!(!env.sys("/home/user/.config/foo").exists());
    assert_eq!(report.freed, plan.total_size);
    // Results stay in plan order; history ids name plan indices, the
    // privileged entries written right after the batch.
    for (i, r) in report.results.iter().enumerate() {
        assert_eq!(r.action, plan.actions[i].action);
        assert_eq!(r.history_id, format!("{}-{i}", report.batch));
    }
    let (hist, _) = read_history(&env).unwrap();
    let ids: Vec<&str> = hist.iter().map(|h| h.id.as_str()).collect();
    let b = &report.batch;
    assert_eq!(
        ids,
        [
            format!("{b}-1"),
            format!("{b}-2"),
            format!("{b}-3"),
            format!("{b}-0")
        ]
    );
    assert_eq!(hist[0].action, HistoryAction::PrivilegedCommand);
    assert_eq!(hist[0].path, Path::new("/usr/bin/flatpak"));
    assert_eq!(hist[1].action, HistoryAction::PackageRemove);
    assert_eq!(hist[2].action, HistoryAction::Delete);
    assert_eq!(
        hist[2].path,
        Path::new("/var/lib/systemd/coredump/core.x.1")
    );
    assert_eq!(hist[3].action, HistoryAction::Trash);
}

#[test]
fn privileged_step_failure_reports_last_output_line() {
    let t = FixtureTree::new();
    let log = t.dir.path().join("calls.log");
    logging_program(
        &t,
        "pacman",
        &log,
        "checking...\nerror: target not found: foo\n",
        1,
    );
    logging_program(&t, "flatpak", &log, "", 3);
    logging_program(&t, "true", &log, "", 0);
    let env = t.env();
    let plan = plan_of(vec![
        pkg_remove(&["foo"], &["1.0-1"]),
        priv_cmd("flatpak", &["x"]),
        priv_cmd("true", &[]),
    ]);
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
        Some("pacman exited with status 1: error: target not found: foo; still installed: foo")
    );
    assert_eq!(
        report.results[1].error.as_deref(),
        Some("exited with status 3")
    );
    assert!(report.results[2].ok);
    assert!(!report.escalation_failed);
    assert_eq!(report.freed, 0);
}

#[test]
fn privileged_action_with_dependency_is_skipped() {
    let t = FixtureTree::new();
    let log = t.dir.path().join("calls.log");
    logging_program(&t, "flatpak", &log, "", 0);
    t.write("home/user/.config/foo/a", b"a");
    let env = t.env();
    let mut cmd = priv_cmd("flatpak", &["x"]);
    cmd.depends_on = Some(0);
    let plan = plan_of(vec![planned(&env, trash("/home/user/.config/foo")), cmd]);
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
    assert!(report.results[0].ok);
    assert!(!report.results[1].ok);
    assert!(
        report.results[1]
            .error
            .as_deref()
            .unwrap()
            .starts_with("skipped: "),
        "{:?}",
        report.results[1]
    );
    assert_eq!(*pr.calls.lock().unwrap(), 0);
}

#[test]
fn privileged_cleanup_reports_measured_space() {
    let t = FixtureTree::new();
    let log = t.dir.path().join("calls.log");
    logging_program(&t, "paccache", &log, "", 0);
    let old = t.write("var/cache/pacman/pkg/a-1.pkg.tar.zst", &[1u8; 20_000]);
    t.write("var/cache/pacman/pkg/a-2.pkg.tar.zst", &[1u8; 20_000]);
    let env = t.env();
    let gone =
        crate::size::disk_usage(&env, Path::new("/var/cache/pacman/pkg/a-1.pkg.tar.zst")).bytes;
    let mut pa = priv_cmd("paccache", &["-rk1"]);
    pa.size = 999_999; // the scan's estimate
    let report = execute_full(
        &env,
        &Config::default(),
        &plan_of(vec![pa]),
        &FakeRunner::new(),
        &RemovingRunner {
            remove: vec![old],
            out: b"@@crumbs 0 0\n".to_vec(),
        },
        Some(&esc()),
        &no_post_check,
    );
    assert!(report.results[0].ok, "{:?}", report.results[0]);
    assert_eq!(report.results[0].freed, Some(gone));
    assert_eq!(report.freed, gone);
}

#[test]
fn escalation_denied_marks_all_failed_and_skips_dependents() {
    let t = FixtureTree::new();
    let log = t.dir.path().join("calls.log");
    logging_program(&t, "pacman", &log, "", 0);
    logging_program(&t, "flatpak", &log, "", 0);
    t.write("home/user/.config/foo/a", b"a");
    let env = t.env();
    let mut leftover = planned(&env, trash("/home/user/.config/foo"));
    leftover.depends_on = Some(0);
    let plan = plan_of(vec![
        pkg_remove(&["foo"], &["1.0-1"]),
        leftover,
        priv_cmd("flatpak", &["uninstall", "--system", "-y", "org.x.App"]),
    ]);
    let pr = DeniedRunner::default();
    let report = execute_full(
        &env,
        &Config::default(),
        &plan,
        &FakeRunner::new(),
        &pr,
        Some(&esc()),
        // Nothing ran, so the package is still there.
        &|_, _, names| Ok(names.to_vec()),
    );
    assert_eq!(*pr.calls.lock().unwrap(), 1);
    assert!(report.escalation_failed);
    assert_eq!(
        report.results[0].error.as_deref(),
        Some("privilege escalation was cancelled or denied; still installed: foo")
    );
    assert_eq!(report.results[2].error.as_deref(), Some(ESCALATION_DENIED));
    assert!(report.results.iter().all(|r| !r.ok));
    let r = &report.results[1];
    assert!(!r.ok);
    assert!(
        r.error.as_deref().unwrap().starts_with("skipped: "),
        "{r:?}"
    );
    assert!(env.sys("/home/user/.config/foo/a").exists());
    assert!(!log.exists());
    assert_eq!(report.freed, 0);
    let (hist, _) = read_history(&env).unwrap();
    assert_eq!(hist.len(), 3);
    assert!(hist.iter().all(|h| !h.ok));
}
