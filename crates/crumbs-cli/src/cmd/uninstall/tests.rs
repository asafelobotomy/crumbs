use super::*;
use crumbs_core::leftovers::Confidence;
use crumbs_core::plan::{Action, build_plan};
use crumbs_core::safety::{Fingerprint, Scope};
use crumbs_core::size::DiskUsage;
use crumbs_core::testing::FixtureTree;

fn finding(path: &str, kind: FindingKind) -> Finding {
    Finding {
        path: PathBuf::from(path),
        kind,
        confidence: Confidence::High,
        shared: false,
        reason: String::new(),
        size: DiskUsage::default(),
        preselected: true,
        scope: Scope::Home,
        fingerprint: Fingerprint {
            dev: 0,
            ino: 0,
            is_symlink: false,
            ctime: 0,
            ctime_nsec: 0,
        },
        mime_lines: Vec::new(),
    }
}

fn removal_plan(n: usize) -> RemovalPlan {
    RemovalPlan {
        actions: (0..n)
            .map(|k| Action::PrivilegedCommand {
                program: "flatpak".into(),
                args: vec![k.to_string()],
            })
            .collect(),
        ..RemovalPlan::default()
    }
}

#[test]
fn leftovers_depend_on_every_removal_action() {
    let t = FixtureTree::new();
    let env = t.env();
    let app = App::new(SourceKind::Flatpak, "org.x.App");
    let mut f = finding("/home/user/.var/app/org.x.App", FindingKind::Data);
    f.size = DiskUsage::default();
    let mut plan = build_plan(&env, Some(&app), &[f.clone(), f], &PlanOptions::default()).unwrap();
    attach_removal(&mut plan, removal_plan(2));
    let deps: Vec<Option<usize>> = plan.actions.iter().map(|a| a.depends_on).collect();
    // The second removal waits for the first; leftovers for the last.
    assert_eq!(deps, [None, Some(0), Some(1), Some(1)]);
    assert!(!plan.notes.iter().any(|n| n == NOTE_APP_STAYS));
    assert!(plan.notes.iter().any(|n| n == NOTE_NO_UNDO));
}

#[test]
fn pacman_removal_with_picked_etc_leftover_runs_in_one_batch() {
    use crumbs_core::config::Config;
    use crumbs_core::executor::execute_full;
    use crumbs_core::privilege::{Escalation, EscalationKind};
    use crumbs_core::size::disk_usage;
    use crumbs_core::testing::{FakePrivilegedRunner, FakeRunner};
    use std::os::unix::fs::PermissionsExt;

    let t = FixtureTree::new();
    t.pacman_pkg("inky", "1.0-1", Some(1), &["usr/bin/inky"]);
    t.write_exec("usr/bin/pacman", b"#!/bin/sh\nexit 0\n");
    t.write_exec("usr/bin/rm", b"#!/bin/sh\nexec /usr/bin/rm \"$@\"\n");
    t.write_exec("usr/bin/sh", b"#!/bin/sh\nexec /bin/sh \"$@\"\n");
    t.write_exec(
        "usr/bin/readlink",
        b"#!/bin/sh\nexec /usr/bin/readlink \"$@\"\n",
    );
    t.write(
        "proc/self/mountinfo",
        b"36 35 98:0 /mnt1 / rw,noatime master:1 - ext3 /dev/root rw\n",
    );
    t.write("etc/inky/inky.conf", b"x");
    t.write("home/user/.cache/inky/blob", b"b");
    for rel in ["", "etc"] {
        std::fs::set_permissions(t.dir.path().join(rel), PermissionsExt::from_mode(0o755)).unwrap();
    }
    let mut env = t.env();
    env.set_system_owner_uid(env.uid);
    let scanned = |path: &str, kind, scope| {
        let mut f = finding(path, kind);
        f.scope = scope;
        f.size = disk_usage(&env, Path::new(path));
        f.fingerprint = safety::fingerprint(&env, Path::new(path)).unwrap();
        f
    };
    let app = App::new(SourceKind::Pacman, "pacman:inky");
    let findings = [
        scanned("/home/user/.cache/inky", FindingKind::Cache, Scope::Home),
        scanned("/etc/inky", FindingKind::SystemConfig, Scope::SystemTree),
    ];
    let mut plan = build_plan(&env, Some(&app), &findings, &PlanOptions::default()).unwrap();
    let removal = RemovalPlan {
        actions: vec![Action::PackageRemove {
            source: SourceKind::Pacman,
            names: vec!["inky".into()],
            versions: vec!["1.0-1".into()],
            program: "pacman".into(),
            args: vec!["-Rns".into(), "--noconfirm".into(), "inky".into()],
        }],
        ..RemovalPlan::default()
    };
    attach_removal(&mut plan, removal);
    let deps: Vec<Option<usize>> = plan.actions.iter().map(|a| a.depends_on).collect();
    // The home leftover waits for the package; the picked system
    // leftover for nothing.
    assert_eq!(deps, [None, Some(0), None]);
    let pr = FakePrivilegedRunner::default();
    let esc = Escalation {
        kind: EscalationKind::Pkexec,
        program: "/usr/bin/pkexec".into(),
        requires_tty: false,
    };
    let report = execute_full(
        &env,
        &Config::default(),
        &plan,
        &FakeRunner::new(),
        &pr,
        Some(&esc),
        &|_, _, _| Ok(Vec::new()),
    );
    for r in &report.results {
        assert!(r.ok, "{r:?}");
    }
    assert_eq!(pr.calls.lock().unwrap().len(), 1, "one batch");
    assert!(!env.sys("/etc/inky").exists());
    assert!(!env.sys("/home/user/.cache/inky").exists());
}

#[test]
fn launcher_running_the_app_file_is_its_own() {
    let t = FixtureTree::new();
    let env = t.env();
    t.desktop(
        "/home/user/.local/share/applications/runs-it.desktop",
        "[Desktop Entry]\nType=Application\nName=X\nExec=/home/user/Applications/X.AppImage %U\n",
    );
    t.desktop(
        "/home/user/.local/share/applications/tilde.desktop",
        "[Desktop Entry]\nType=Application\nName=X\nExec=~/Applications/X.AppImage\n",
    );
    t.desktop(
        "/home/user/.local/share/applications/other.desktop",
        "[Desktop Entry]\nType=Application\nName=X\nExec=/usr/bin/x\n",
    );
    let mut app = App::new(SourceKind::AppImage, "X.AppImage");
    app.location = Some(PathBuf::from("/home/user/Applications/X.AppImage"));
    assert!(app.desktop_ids.is_empty());
    let own = |rel: &str, kind| is_own_launcher(&env, &app, &finding(rel, kind));
    assert!(own(
        "/home/user/.local/share/applications/runs-it.desktop",
        FindingKind::DesktopEntry
    ));
    assert!(own(
        "/home/user/.local/share/applications/tilde.desktop",
        FindingKind::DesktopEntry
    ));
    assert!(!own(
        "/home/user/.local/share/applications/other.desktop",
        FindingKind::DesktopEntry
    ));
    assert!(own(
        "/home/user/.config/mimeapps.list",
        FindingKind::MimeAssoc
    ));
    assert!(!own(
        "/home/user/.config/autostart/runs-it.desktop",
        FindingKind::Autostart
    ));
}
