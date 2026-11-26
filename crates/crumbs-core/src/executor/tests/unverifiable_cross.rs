use super::*;

#[test]
fn unverifiable_removal_is_not_ok_and_records_nothing() {
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
        &|_, _, _| Err("cannot read the pacman database".into()),
    );
    assert!(!report.results[0].ok);
    assert_eq!(
        report.results[0].error.as_deref(),
        Some("could not verify removal: cannot read the pacman database")
    );
    assert!(
        report.results[1]
            .error
            .as_deref()
            .unwrap()
            .starts_with("skipped: ")
    );
    assert!(env.sys("/home/user/.config/foo/a").exists());
    let (hist, _) = read_history(&env).unwrap();
    assert!(hist.iter().all(|h| h.packages.is_none()), "{hist:?}");
}

/// An action an earlier plan's action covered waits for it: it is skipped,
/// naming the failed cover, and never run; one whose cover succeeded runs.
#[test]
fn cross_plan_gate_skips_when_cover_failed() {
    let t = FixtureTree::new();
    t.write("home/user/.config/shared/a", b"a");
    t.write("home/user/.config/ok/a", b"a");
    t.write("home/user/.config/dep/a", b"a");
    t.write("home/user/.config/dep2/a", b"a");
    let env = t.env();
    let shared = planned(&env, trash("/home/user/.config/shared"));
    let ok = planned(&env, trash("/home/user/.config/ok"));
    let mut dep = planned(&env, trash("/home/user/.config/dep"));
    dep.depends_on_plan = Some((0, 0));
    let mut dep2 = planned(&env, trash("/home/user/.config/dep2"));
    dep2.depends_on_plan = Some((0, 1));
    // The cover vanishes after the scan, so its action fails.
    fs::remove_dir_all(env.sys("/home/user/.config/shared")).unwrap();
    let plans = [
        app_plan("pacman:foo", vec![shared, ok]),
        app_plan("pacman:bar", vec![dep, dep2]),
    ];
    let reports = execute_many(
        &env,
        &Config::default(),
        &plans,
        &FakeRunner::new(),
        &FakePrivilegedRunner::default(),
        Some(&esc()),
        &no_post_check,
    );
    assert!(!reports[0].results[0].ok);
    assert!(reports[0].results[1].ok);
    let skipped = &reports[1].results[0];
    assert!(!skipped.ok);
    assert!(
        !skipped.rights_missing,
        "a skipped dependent is never listed"
    );
    assert_eq!(
        skipped.error.as_deref(),
        Some("skipped: ~/.config/shared was not removed (it covered this action)")
    );
    assert!(env.sys("/home/user/.config/dep").exists(), "must not run");
    assert!(reports[1].results[1].ok);
    assert!(!env.sys("/home/user/.config/dep2").exists());
}

#[test]
fn home_delete_history_has_no_scope() {
    let t = FixtureTree::new();
    t.write("home/user/.cache/foo/a", b"a");
    let env = t.env();
    let plan = plan_of(vec![planned(&env, delete("/home/user/.cache/foo"))]);
    let report = execute(&env, &Config::default(), &plan);
    assert!(report.results[0].ok, "{:?}", report.results[0]);
    let raw = fs::read_to_string(env.state_home.join("crumbs/history.jsonl")).unwrap();
    assert!(!raw.contains("\"scope\""), "{raw}");
}

#[test]
fn only_root_deletes_record_a_scope() {
    let t = FixtureTree::new();
    t.write("home/user/.local/share/Trash/files/x", b"x");
    let env = t.env();
    let mut pa = planned(&env, delete("/home/user/.local/share/Trash/files/x"));
    pa.scope = Scope::HomeTrash;
    let plan = plan_of(vec![pa]);
    let report = execute(&env, &Config::default(), &plan);
    let raw = fs::read_to_string(env.state_home.join("crumbs/history.jsonl")).unwrap();
    assert!(!raw.contains("\"scope\""), "{report:?} {raw}");
}

#[test]
fn unmanaged_uninstall_with_picked_system_leftover_runs_end_to_end() {
    use crate::leftovers::FindingKind;
    use crate::plan::{PlanOptions, build_plan_unmanaged};
    let (t, env) = system_tree_fixture();
    fs::create_dir_all(t.dir.path().join("etc/inky")).unwrap();
    t.write("etc/inky/inky.conf", b"x");
    fs::set_permissions(t.dir.path().join("etc"), fs::Permissions::from_mode(0o755)).unwrap();
    t.write("home/user/.cache/inky/blob", b"b");
    t.symlink("home/user/.local/bin/inky", "/opt/inky/a");
    let mut app = crate::model::App::new(SourceKind::Unmanaged, "unmanaged:inky.desktop");
    app.program = Some("/opt/inky/a".into());
    let target = crate::removal::UnmanagedTarget {
        path: "/opt/inky".into(),
        scope: Scope::SystemTree,
        fingerprint: fingerprint(&env, Path::new("/opt/inky")).unwrap(),
        size: disk_usage(&env, Path::new("/opt/inky")),
        left_link: None,
        left_link_protected: false,
        home_link: Some((
            "/home/user/.local/bin/inky".into(),
            fingerprint(&env, Path::new("/home/user/.local/bin/inky")).unwrap(),
        )),
    };
    let findings = [
        scanned(
            &env,
            "/home/user/.cache/inky",
            FindingKind::Cache,
            Scope::Home,
        ),
        scanned(
            &env,
            "/etc/inky",
            FindingKind::SystemConfig,
            Scope::SystemTree,
        ),
    ];
    let plan = build_plan_unmanaged(
        &env,
        Some(&app),
        Some(&target),
        &findings,
        &PlanOptions::default(),
    )
    .unwrap();
    let deps: Vec<Option<usize>> = plan.actions.iter().map(|a| a.depends_on).collect();
    // Home actions wait for the target; the picked system leftover waits
    // for nothing.
    assert_eq!(deps, [None, Some(0), Some(0), None]);
    let (report, calls) = run_tree(&env, &plan);
    assert_eq!(calls.len(), 1, "one batch");
    assert_eq!(step_count(&calls[0]), 2);
    for r in &report.results {
        assert!(r.ok, "{r:?}");
    }
    assert!(!env.sys("/opt/inky").exists());
    assert!(!env.sys("/etc/inky").exists());
    assert!(!env.sys("/home/user/.cache/inky").exists());
    assert!(fs::symlink_metadata(env.sys("/home/user/.local/bin/inky")).is_err());
}

#[test]
fn measured_dirs_of_known_cleanup_commands() {
    let t = FixtureTree::new();
    let env = t.env();
    let dirs = |program: &str, args: &[&str]| {
        measured_dirs(
            &env,
            &Action::PrivilegedCommand {
                program: program.into(),
                args: args.iter().map(|a| (*a).to_string()).collect(),
            },
        )
    };
    let p = |s: &str| vec![PathBuf::from(s)];
    assert_eq!(dirs("paccache", &["-rk2"]), p("/var/cache/pacman/pkg"));
    assert_eq!(
        dirs("pacman", &["-Sc", "--noconfirm"]),
        p("/var/cache/pacman/pkg")
    );
    assert_eq!(
        dirs("journalctl", &["--vacuum-size=500M"]),
        p("/var/log/journal")
    );
    assert_eq!(dirs("apt-get", &["clean"]), p("/var/cache/apt/archives"));
    assert_eq!(
        dirs("dnf", &["clean", "packages"]),
        vec![
            PathBuf::from("/var/cache/dnf"),
            PathBuf::from("/var/cache/libdnf5")
        ]
    );
    assert_eq!(
        dirs(
            "systemd-tmpfiles",
            &["--clean", "--prefix=/tmp", "--prefix=/var/tmp"]
        ),
        vec![PathBuf::from("/tmp"), PathBuf::from("/var/tmp")]
    );
    assert_eq!(
        dirs(
            "flatpak",
            &[
                "uninstall",
                "--system",
                "--unused",
                "-y",
                "--noninteractive"
            ]
        ),
        p("/var/lib/flatpak")
    );
    let user = measured_dirs(
        &env,
        &Action::UserCommand {
            program: "flatpak".into(),
            args: ["uninstall", "--user", "--unused", "--noninteractive", "-y"]
                .map(String::from)
                .to_vec(),
        },
    );
    assert_eq!(user, p("/home/user/.local/share/flatpak"));
    let engine = |program: &str, args: &[&str]| {
        measured_dirs(
            &env,
            &Action::UserCommand {
                program: program.into(),
                args: args.iter().map(|a| (*a).to_string()).collect(),
            },
        )
    };
    // Container storage is not measured: walking it is slow, and the
    // images' own sizes say what goes.
    assert!(engine("podman", &["image", "prune", "-f"]).is_empty());
    assert!(engine("podman", &["volume", "rm", "cache"]).is_empty());
    assert!(engine("docker", &["image", "prune", "-f"]).is_empty());
    // Anything else is not measured: an app's uninstall, an unknown tool.
    assert!(dirs("flatpak", &["uninstall", "--system", "-y", "org.app"]).is_empty());
    assert!(dirs("snap", &["remove", "--purge", "x"]).is_empty());
}

#[test]
fn unmeasurable_cleanup_keeps_the_estimate() {
    // The folder does not exist: nothing to measure, so the scan's
    // estimate stands and the result carries no measurement.
    let t = FixtureTree::new();
    let log = t.dir.path().join("calls.log");
    logging_program(&t, "journalctl", &log, "", 0);
    let env = t.env();
    let mut pa = priv_cmd("journalctl", &["--vacuum-size=500M"]);
    pa.size = 4096;
    let report = execute_full(
        &env,
        &Config::default(),
        &plan_of(vec![pa]),
        &FakeRunner::new(),
        &CannedRunner(b"@@crumbs 0 0\n".to_vec()),
        Some(&esc()),
        &no_post_check,
    );
    assert!(report.results[0].ok, "{:?}", report.results[0]);
    assert_eq!(report.results[0].freed, None);
    assert_eq!(report.freed, 4096);
}

#[test]
fn measurement_tolerates_unreadable_parts_that_stay() {
    // pacman 7 leaves root-only download-* folders in the cache: they are
    // unreadable before and after, so what was read is still compared.
    use std::os::unix::fs::PermissionsExt;
    let t = FixtureTree::new();
    logging_program(&t, "paccache", &t.dir.path().join("l.log"), "", 0);
    let old = t.write("var/cache/pacman/pkg/a-1.pkg.tar.zst", &[1u8; 20_000]);
    t.write("var/cache/pacman/pkg/a-2.pkg.tar.zst", &[1u8; 20_000]);
    let locked = t.mkdir("var/cache/pacman/pkg/download-Xy12");
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
    let env = t.env();
    let gone =
        crate::size::disk_usage(&env, Path::new("/var/cache/pacman/pkg/a-1.pkg.tar.zst")).bytes;
    let report = execute_full(
        &env,
        &Config::default(),
        &plan_of(vec![priv_cmd("paccache", &["-rk1"])]),
        &FakeRunner::new(),
        &RemovingRunner {
            remove: vec![old],
            out: b"@@crumbs 0 0\n".to_vec(),
        },
        Some(&esc()),
        &no_post_check,
    );
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(report.results[0].ok, "{:?}", report.results[0]);
    assert_eq!(report.results[0].freed, Some(gone));
}
