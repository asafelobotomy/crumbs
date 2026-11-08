use super::*;
use crate::command::CmdOutput;
use crate::model::Installation;
use crate::testing::{FakeRunner, FixtureTree};

fn out(status: i32, stdout: &str, stderr: &str) -> CmdOutput {
    CmdOutput {
        status,
        stdout: stdout.into(),
        stderr: stderr.into(),
        timed_out: false,
    }
}

fn sim(runner: FakeRunner, pkg: &str, o: CmdOutput) -> FakeRunner {
    runner.on(
        "pacman",
        &["-Rs", "--print", "--print-format", "%n %v", pkg],
        o,
    )
}

fn pacman_app(name: &str) -> App {
    let mut app = App::new(SourceKind::Pacman, name);
    app.version = Some("0.24.5-1".into());
    app
}

fn with_pacman() -> FixtureTree {
    let t = FixtureTree::new();
    t.write_exec("/usr/bin/pacman", b"");
    t.mkdir("/var/lib/pacman/local");
    t
}

fn flatpak_app(installation: Installation) -> App {
    let mut app = App::new(SourceKind::Flatpak, "org.x.Y");
    app.installation = installation;
    app
}

#[test]
fn pacman_simulation_lists_extras() {
    let t = with_pacman();
    let runner = sim(
        FakeRunner::new(),
        "libdatachannel",
        out(
            0,
            "libdatachannel 0.24.5-2\nlibjuice 1.7.3-1\ngst-plugin-gtk4 0.15.4-1\ngn 0.2385.9ece3f52-1\n",
            "",
        ),
    );
    let plan = plan_removal(&t.env(), &runner, &pacman_app("libdatachannel")).unwrap();
    assert_eq!(
        plan.actions,
        [Action::PackageRemove {
            source: SourceKind::Pacman,
            names: vec![
                "libdatachannel".into(),
                "libjuice".into(),
                "gst-plugin-gtk4".into(),
                "gn".into()
            ],
            versions: vec![
                "0.24.5-2".into(),
                "1.7.3-1".into(),
                "0.15.4-1".into(),
                "0.2385.9ece3f52-1".into()
            ],
            program: "pacman".into(),
            args: [
                "-Rn",
                "--noconfirm",
                "libdatachannel",
                "libjuice",
                "gst-plugin-gtk4",
                "gn"
            ]
            .map(String::from)
            .to_vec(),
        }]
    );
    assert_eq!(
        plan.also_removed,
        [
            ("libjuice".to_string(), "1.7.3-1".to_string()),
            ("gst-plugin-gtk4".to_string(), "0.15.4-1".to_string()),
            ("gn".to_string(), "0.2385.9ece3f52-1".to_string()),
        ]
    );
    assert_eq!(runner.calls().len(), 1);
}

#[test]
fn pacman_simulation_refusal_text() {
    let t = with_pacman();
    // Real pacman: the reason on stdout, the summary on stderr.
    let stderr = "error: failed to prepare transaction (could not satisfy dependencies)\n";
    let stdout = ":: removing libjuice breaks dependency 'libjuice' required by libdatachannel\nnot a reason line\n";
    let runner = sim(FakeRunner::new(), "libjuice", out(1, stdout, stderr));
    let err = plan_removal(&t.env(), &runner, &pacman_app("libjuice")).unwrap_err();
    assert_eq!(
        err,
        RemovalError::Refused(
            "error: failed to prepare transaction (could not satisfy dependencies)\n:: removing libjuice breaks dependency 'libjuice' required by libdatachannel".into()
        )
    );
    assert!(err.to_string().contains("breaks dependency 'libjuice'"));
}

#[test]
fn pacman_busy_lock() {
    let t = with_pacman();
    t.write("/var/lib/pacman/db.lck", b"");
    // No simulation is registered: the lock check must come first.
    let runner = FakeRunner::new();
    let err = plan_removal(&t.env(), &runner, &pacman_app("foo")).unwrap_err();
    assert_eq!(
        err,
        RemovalError::Busy("pacman is busy (database lock present)".into())
    );
    assert!(runner.calls().is_empty());
}

#[test]
fn dangling_symlink_lock_is_busy() {
    let t = with_pacman();
    t.symlink("/var/lib/pacman/db.lck", "/nonexistent");
    let runner = FakeRunner::new();
    let err = plan_removal(&t.env(), &runner, &pacman_app("foo")).unwrap_err();
    assert!(matches!(err, RemovalError::Busy(_)));
}

#[test]
fn invalid_package_name_refused() {
    let t = with_pacman();
    let runner = FakeRunner::new();
    for bad in ["a b", "a;rm", "", "-Rdd", "a\nb", "naïve"] {
        let err = plan_removal(&t.env(), &runner, &pacman_app(bad)).unwrap_err();
        assert!(matches!(err, RemovalError::InvalidName(_)), "{bad:?}");
    }
    assert!(runner.calls().is_empty());

    // A name pacman prints that is odd is refused too.
    for line in [
        "foo 1\nbad;name 1\n",
        "foo\n",
        "foo 1 extra\n",
        "foo 1\n-evil 2\n",
    ] {
        let runner = sim(FakeRunner::new(), "foo", out(0, line, ""));
        let err = plan_removal(&t.env(), &runner, &pacman_app("foo")).unwrap_err();
        assert!(matches!(err, RemovalError::InvalidName(_)), "{line:?}");
    }
    // Valid special characters pass.
    let runner = sim(
        FakeRunner::new(),
        "g++",
        out(0, "g++ 1\nlib@x_y.z+1-2 3\n", ""),
    );
    assert!(plan_removal(&t.env(), &runner, &pacman_app("g++")).is_ok());
}

#[test]
fn flatpak_user_system_custom_commands() {
    let t = FixtureTree::new();
    let runner = FakeRunner::new();
    let run = |i| {
        plan_removal(&t.env(), &runner, &flatpak_app(i))
            .unwrap()
            .actions
    };
    let args = |flag: &str| {
        ["uninstall", flag, "-y", "--noninteractive", "org.x.Y"]
            .map(String::from)
            .to_vec()
    };
    assert_eq!(
        run(Installation::User),
        [Action::UserCommand {
            program: "flatpak".into(),
            args: args("--user")
        }]
    );
    assert_eq!(
        run(Installation::System),
        [Action::PrivilegedCommand {
            program: "flatpak".into(),
            args: args("--system")
        }]
    );
    assert_eq!(
        run(Installation::Custom("my-ssd".into())),
        [Action::PrivilegedCommand {
            program: "flatpak".into(),
            args: args("--installation=my-ssd")
        }]
    );
    let bad = plan_removal(
        &t.env(),
        &runner,
        &flatpak_app(Installation::Custom("a b".into())),
    );
    assert!(matches!(bad, Err(RemovalError::InvalidName(_))));
    assert!(runner.calls().is_empty());
}

#[test]
fn unmanaged_unsupported_and_appimage_unchanged() {
    let t = FixtureTree::new();
    let runner = FakeRunner::new();
    let err = plan_removal(&t.env(), &runner, &App::new(SourceKind::Unmanaged, "x")).unwrap_err();
    assert_eq!(
        err,
        RemovalError::Unsupported(
            "removing unmanaged apps isn't supported yet; use --leftovers-only".into()
        )
    );
    let plan = plan_removal(&t.env(), &runner, &App::new(SourceKind::AppImage, "x")).unwrap();
    assert!(plan.actions.is_empty());
}

#[test]
fn still_installed_reads_db() {
    let t = with_pacman();
    t.pacman_pkg("foo", "1.0-1", None, &[]);
    t.pacman_pkg("bar", "2.0-1", None, &[]);
    t.write("/var/lib/pacman/local/ALPM_DB_VERSION", b"9\n");
    let names: Vec<String> = ["foo", "gone", "bar"].map(String::from).to_vec();
    let runner = FakeRunner::new();
    let got = still_installed(&t.env(), &runner, SourceKind::Pacman, &names);
    assert_eq!(got, Ok(vec!["foo".to_string(), "bar".into()]));
    std::fs::remove_dir_all(t.dir.path().join("var/lib/pacman/local/foo-1.0-1")).unwrap();
    let got = still_installed(&t.env(), &runner, SourceKind::Pacman, &names);
    assert_eq!(got, Ok(vec!["bar".to_string()]));
    assert_eq!(
        still_installed(&t.env(), &runner, SourceKind::Unmanaged, &names),
        Ok(Vec::new())
    );
}

#[test]
fn pacman_still_installed_fails_closed() {
    let names = vec!["foo".to_string()];
    let runner = FakeRunner::new();
    // No database at all.
    let t = FixtureTree::new();
    let e = still_installed(&t.env(), &runner, SourceKind::Pacman, &names).unwrap_err();
    assert!(e.contains("cannot read"), "{e}");
    // An entry without a desc.
    let t = with_pacman();
    t.write("/var/lib/pacman/local/foo-1.0-1/files", b"%FILES%\n");
    let e = still_installed(&t.env(), &runner, SourceKind::Pacman, &names).unwrap_err();
    assert!(e.contains("foo-1.0-1/desc"), "{e}");
    // A desc without a name.
    t.write("/var/lib/pacman/local/foo-1.0-1/desc", b"%VERSION%\n1\n");
    let e = still_installed(&t.env(), &runner, SourceKind::Pacman, &names).unwrap_err();
    assert!(e.contains("no %NAME%"), "{e}");
}

#[test]
fn flatpak_still_installed_lists_apps() {
    let t = FixtureTree::new();
    t.write_exec("/usr/bin/flatpak", b"");
    let runner = FakeRunner::new().on(
        "flatpak",
        &["list", "--app", "--columns=application"],
        out(0, "org.x.Y\norg.other.Z\n", ""),
    );
    let names: Vec<String> = ["org.x.Y", "org.gone.G"].map(String::from).to_vec();
    assert_eq!(
        still_installed(&t.env(), &runner, SourceKind::Flatpak, &names),
        Ok(vec!["org.x.Y".to_string()])
    );
}

#[test]
fn flatpak_still_installed_fails_closed() {
    let names = vec!["org.x.Y".to_string()];
    // Missing binary.
    let t = FixtureTree::new();
    let r = still_installed(&t.env(), &FakeRunner::new(), SourceKind::Flatpak, &names);
    assert!(r.unwrap_err().contains("not installed"));
    // Runner failure and non-zero exit.
    t.write_exec("/usr/bin/flatpak", b"");
    assert!(still_installed(&t.env(), &FakeRunner::new(), SourceKind::Flatpak, &names).is_err());
    let runner = FakeRunner::new().on(
        "flatpak",
        &["list", "--app", "--columns=application"],
        out(1, "", "boom\n"),
    );
    assert_eq!(
        still_installed(&t.env(), &runner, SourceKind::Flatpak, &names),
        Err("boom".into())
    );
}

// ---- unmanaged apps ----

mod unmanaged {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};

    use super::*;
    use crate::config::Config;
    use crate::env::Env;
    use crate::inventory::Inventory;
    use crate::ownership::OwnedIndex;
    use crate::safety::Scope;

    fn sys_env(t: &FixtureTree) -> Env {
        t.mkdir("opt");
        for rel in ["", "opt"] {
            fs::set_permissions(t.dir.path().join(rel), fs::Permissions::from_mode(0o755)).unwrap();
        }
        let mut env = t.env();
        env.set_system_owner_uid(env.uid);
        env
    }

    fn app(name: &str, program: &str) -> App {
        let mut a = App::new(SourceKind::Unmanaged, &format!("{name}.desktop"));
        a.name = name.into();
        a.display_name = name.into();
        a.desktop_ids.push(format!("{name}.desktop"));
        a.program = Some(PathBuf::from(program));
        a
    }

    fn inv(apps: Vec<App>, owned: &[&str], complete: bool) -> Inventory {
        Inventory {
            apps,
            statuses: Vec::new(),
            errors: Vec::new(),
            owned: OwnedIndex::new(
                owned.iter().map(PathBuf::from),
                Vec::<PathBuf>::new(),
                complete,
            ),
        }
    }

    fn plan(env: &Env, i: &Inventory, a: &App) -> Result<UnmanagedTarget, RemovalError> {
        plan_unmanaged(env, &Config::default(), i, a)
    }

    fn reason(r: Result<UnmanagedTarget, RemovalError>) -> String {
        match r.unwrap_err() {
            RemovalError::Unsupported(m) => m,
            e => panic!("{e:?}"),
        }
    }

    #[test]
    fn unmanaged_opt_dir_planned_as_root_delete() {
        let t = FixtureTree::new();
        t.write_exec("opt/inky/Inky", b"");
        let env = sys_env(&t);
        let a = app("inky", "/opt/inky/Inky");
        let i = inv(vec![a.clone()], &["/usr/bin/true"], true);
        let tg = plan(&env, &i, &a).unwrap();
        assert_eq!(tg.path, Path::new("/opt/inky"));
        assert_eq!(tg.scope, Scope::SystemTree);
        assert_eq!(tg.left_link, None);
        assert_eq!(tg.size.files, 1);
    }

    #[test]
    fn unmanaged_vendor_dir_refused() {
        let t = FixtureTree::new();
        t.write_exec("opt/google/chrome/chrome", b"");
        let env = sys_env(&t);
        let a = app("google-chrome-dev", "/opt/google/chrome/chrome");
        let i = inv(vec![a.clone()], &["/usr/bin/true"], true);
        let m = reason(plan(&env, &i, &a));
        assert_eq!(
            m,
            "removing this unmanaged app's files isn't possible: /opt/google does not match the app's name; use --leftovers-only"
        );
    }

    #[test]
    fn unmanaged_dir_shared_by_two_apps_refused() {
        let t = FixtureTree::new();
        t.write_exec("opt/suite/a", b"");
        t.write_exec("opt/suite/b", b"");
        let env = sys_env(&t);
        let a = app("suite", "/opt/suite/a");
        let b = app("other", "/opt/suite/b");
        let i = inv(vec![a.clone(), b], &["/usr/bin/true"], true);
        let m = reason(plan(&env, &i, &a));
        assert!(m.contains("/opt/suite is shared with other"), "{m}");
    }

    #[test]
    fn unmanaged_owned_dir_refused() {
        let t = FixtureTree::new();
        t.write_exec("opt/inky/Inky", b"");
        let env = sys_env(&t);
        let a = app("inky", "/opt/inky/Inky");
        let i = inv(vec![a.clone()], &["/opt/inky/data"], true);
        assert!(reason(plan(&env, &i, &a)).contains("/opt/inky belongs to a package"));
    }

    #[test]
    fn unmanaged_tmpfiles_declared_dir_refused() {
        let t = FixtureTree::new();
        t.write_exec("opt/inky/Inky", b"");
        t.write("usr/lib/tmpfiles.d/inky.conf", b"d /opt/inky/run 0755\n");
        let env = sys_env(&t);
        let a = app("inky", "/opt/inky/Inky");
        let i = inv(vec![a.clone()], &["/usr/bin/true"], true);
        assert!(reason(plan(&env, &i, &a)).contains("/opt/inky belongs to a package"));
    }

    #[test]
    fn unmanaged_opt_without_database_refused_but_home_allowed() {
        let t = FixtureTree::new();
        t.write_exec("opt/inky/Inky", b"");
        t.write_exec("home/user/opt/foo/foo", b"");
        let env = sys_env(&t);
        let a = app("inky", "/opt/inky/Inky");
        for complete in [true, false] {
            let own: &[&str] = if complete { &[] } else { &["/usr/bin/true"] };
            let i = inv(vec![a.clone()], own, complete);
            let m = reason(plan(&env, &i, &a));
            assert!(
                m.contains("package ownership is unknown (no pacman database)"),
                "{m}"
            );
        }
        let h = app("foo", "/home/user/opt/foo/foo");
        let i = inv(vec![h.clone()], &[], false);
        assert!(plan(&env, &i, &h).is_ok());
    }

    #[test]
    fn unmanaged_home_dir_trashed() {
        let t = FixtureTree::new();
        t.write_exec("home/user/opt/foo/foo", b"");
        let env = sys_env(&t);
        let a = app("foo", "/home/user/opt/foo/foo");
        let i = inv(vec![a.clone()], &["/usr/bin/true"], true);
        let tg = plan(&env, &i, &a).unwrap();
        assert_eq!(tg.path, Path::new("/home/user/opt/foo"));
        assert_eq!(tg.scope, Scope::Home);
    }

    #[test]
    fn unmanaged_local_bin_file_trashed() {
        let t = FixtureTree::new();
        t.write_exec("home/user/.local/bin/foo", b"");
        let env = sys_env(&t);
        let a = app("foo", "/home/user/.local/bin/foo");
        let i = inv(vec![a.clone()], &["/usr/bin/true"], true);
        let tg = plan(&env, &i, &a).unwrap();
        assert_eq!(tg.path, Path::new("/home/user/.local/bin/foo"));
        assert_eq!(tg.scope, Scope::Home);
        let other = app("bar", "/home/user/.local/bin/foo");
        let i = inv(vec![other.clone()], &["/usr/bin/true"], true);
        assert!(reason(plan(&env, &i, &other)).contains("does not match the app's name"));
    }

    #[test]
    fn unmanaged_usr_local_refused() {
        let t = FixtureTree::new();
        t.write_exec("usr/local/bin/foo", b"");
        let env = sys_env(&t);
        let a = app("foo", "/usr/local/bin/foo");
        let i = inv(vec![a.clone()], &["/usr/bin/true"], true);
        assert_eq!(
            reason(plan(&env, &i, &a)),
            "removing this unmanaged app's files isn't possible: its program is in a protected or shared location; use --leftovers-only"
        );
    }

    #[test]
    fn unmanaged_symlinked_program_notes_link() {
        let t = FixtureTree::new();
        t.write_exec("opt/inky/Inky", b"");
        t.symlink("usr/bin/inky", "/opt/inky/Inky");
        let env = sys_env(&t);
        let mut a = app("inky", "/opt/inky/Inky");
        a.program_link = Some("/usr/bin/inky".into());
        let i = inv(vec![a.clone()], &["/usr/bin/true"], true);
        let tg = plan(&env, &i, &a).unwrap();
        assert_eq!(tg.path, Path::new("/opt/inky"));
        assert_eq!(tg.left_link.as_deref(), Some(Path::new("/usr/bin/inky")));
        assert!(tg.home_link.is_none());
    }

    #[test]
    fn unmanaged_home_symlink_planned_for_removal() {
        let t = FixtureTree::new();
        t.write_exec("opt/inky/Inky", b"");
        t.symlink("home/user/.local/bin/inky", "/opt/inky/Inky");
        let env = sys_env(&t);
        let mut a = app("inky", "/opt/inky/Inky");
        a.program_link = Some("/home/user/.local/bin/inky".into());
        let i = inv(vec![a.clone()], &["/usr/bin/true"], true);
        let tg = plan(&env, &i, &a).unwrap();
        let (link, fp) = tg.home_link.unwrap();
        assert_eq!(link, Path::new("/home/user/.local/bin/inky"));
        assert!(fp.is_symlink);
        assert!(tg.left_link.is_none());
    }
}
