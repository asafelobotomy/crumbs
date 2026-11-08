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
