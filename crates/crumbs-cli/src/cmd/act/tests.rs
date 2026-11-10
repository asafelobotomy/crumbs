use super::*;
use crumbs_core::safety::{Fingerprint, Scope};
use std::path::{Path, PathBuf};

fn action(a: Action, size: u64) -> PlannedAction {
    PlannedAction {
        action: a,
        size,
        fingerprint: Fingerprint {
            dev: 0,
            ino: 0,
            is_symlink: false,
            ctime: 0,
            ctime_nsec: 0,
        },
        scope: Scope::Home,
        depends_on: None,
        depends_on_plan: None,
        size_known: true,
        reason: String::new(),
    }
}

fn trash(p: &str, size: u64) -> PlannedAction {
    action(Action::Trash { path: p.into() }, size)
}

fn edit(p: &str, line: &str) -> PlannedAction {
    action(
        Action::EditFile {
            path: p.into(),
            remove_lines: vec![line.into()],
        },
        0,
    )
}

fn target(actions: Vec<PlannedAction>) -> Target {
    let total_size = actions.iter().map(|a| a.size).sum();
    Target {
        heading: String::new(),
        app: None,
        plan: Plan {
            app_id: None,
            kind: None,
            actions,
            total_size,
            notes: Vec::new(),
        },
    }
}

fn paths(t: &Target) -> Vec<PathBuf> {
    t.plan
        .actions
        .iter()
        .filter_map(|a| a.action.path().map(Path::to_path_buf))
        .collect()
}

#[test]
fn checklist_item_marks_shared_and_root() {
    let t = crumbs_core::testing::FixtureTree::new();
    let env = t.env();
    let mut f = Finding {
        path: "/home/user/.config/x".into(),
        kind: FindingKind::Config,
        confidence: crumbs_core::leftovers::Confidence::High,
        shared: false,
        reason: String::new(),
        size: crumbs_core::size::DiskUsage::default(),
        preselected: false,
        scope: Scope::Home,
        fingerprint: Fingerprint {
            dev: 0,
            ino: 0,
            is_symlink: false,
            ctime: 0,
            ctime_nsec: 0,
        },
        mime_lines: Vec::new(),
    };
    let plain = checklist_item(&env, &f);
    assert!(
        !plain.contains("(root)") && !plain.contains("(shared)"),
        "{plain}"
    );
    f.scope = Scope::SystemTree;
    f.path = "/opt/x".into();
    assert!(checklist_item(&env, &f).ends_with("  (root)"));
    f.shared = true;
    assert!(checklist_item(&env, &f).ends_with("  (shared)  (root)"));
}

#[test]
fn drop_overlaps_ignores_commands() {
    let cmd = || {
        action(
            Action::UserCommand {
                program: "flatpak".into(),
                args: Vec::new(),
            },
            0,
        )
    };
    let mut ts = [
        target(vec![cmd(), trash("/home/user/flatpak", 1)]),
        target(vec![cmd()]),
    ];
    drop_overlaps(&mut ts);
    assert_eq!(ts[0].plan.actions.len(), 2);
    assert_eq!(ts[1].plan.actions.len(), 1, "a command is never an overlap");
}

#[test]
fn drop_overlaps_plans_each_path_once() {
    let mimeapps = "/home/user/.config/mimeapps.list";
    let mut ts = [
        target(vec![
            trash("/home/user/.config/shared", 10),
            edit(mimeapps, "a=a.desktop"),
        ]),
        target(vec![
            trash("/home/user/.config/shared", 10),
            trash("/home/user/.config/shared/sub", 3),
            trash("/home/user/.config/sharedish", 5),
            edit(mimeapps, "b=b.desktop"),
        ]),
    ];
    drop_overlaps(&mut ts);
    assert_eq!(
        paths(&ts[0]),
        [
            PathBuf::from("/home/user/.config/shared"),
            PathBuf::from(mimeapps)
        ]
    );
    assert_eq!(
        paths(&ts[1]),
        [PathBuf::from("/home/user/.config/sharedish")]
    );
    assert_eq!(ts[1].plan.total_size, 5);
    assert_eq!(ts[0].plan.total_size, 10);
    assert_eq!(
        ts[0].plan.actions[1].action,
        Action::EditFile {
            path: mimeapps.into(),
            remove_lines: vec!["a=a.desktop".into(), "b=b.desktop".into()],
        }
    );
}

#[test]
fn drop_overlaps_remaps_depends_on() {
    let mut dependent = trash("/home/user/.config/Bar", 4);
    dependent.depends_on = Some(0);
    let mut kept_dep = trash("/home/user/.cache/Bar", 2);
    kept_dep.depends_on = Some(0);
    let mut ts = [
        target(vec![trash("/home/user/.local/share/foo", 10)]),
        target(vec![
            trash("/home/user/.local/share/foo/Bar.AppImage", 1),
            dependent,
            kept_dep,
        ]),
        // The file stays here; indexes shift when an earlier action goes.
        target(vec![
            trash("/home/user/.local/share/foo/Baz", 1),
            trash("/home/user/Apps/Q.AppImage", 1),
            {
                let mut a = trash("/home/user/.config/Q", 1);
                a.depends_on = Some(1);
                a
            },
        ]),
    ];
    drop_overlaps(&mut ts);
    assert_eq!(ts[1].plan.actions.len(), 2);
    // Their dependency went to the earlier plan's covering action: the
    // executor waits for it there.
    for a in &ts[1].plan.actions {
        assert_eq!(a.depends_on, None);
        assert_eq!(a.depends_on_plan, Some((0, 0)));
    }
    assert_eq!(ts[2].plan.actions.len(), 2);
    assert_eq!(ts[2].plan.actions[1].depends_on, Some(0));
    assert_eq!(ts[2].plan.actions[1].depends_on_plan, None);
}
