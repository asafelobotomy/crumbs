use super::*;

#[test]
fn also_removed_lists_every_name() {
    let a = Action::PackageRemove {
        source: crumbs_core::SourceKind::Pacman,
        names: vec!["app".into(), "b".into(), "c".into()],
        versions: vec!["1".into(), "2".into()],
        program: "pacman".into(),
        args: Vec::new(),
    };
    assert_eq!(also_removed(&a).as_deref(), Some("also removed: b 2, c"));
}

#[test]
fn unknown_size_shows_dash_and_unknown_total() {
    use crumbs_core::safety::Fingerprint;
    let mut a = PlannedAction {
        action: Action::Delete {
            path: "/opt/inky".into(),
        },
        size: 0,
        fingerprint: Fingerprint::default(),
        scope: Scope::SystemTree,
        depends_on: None,
        depends_on_plan: None,
        size_known: false,
        reason: String::new(),
    };
    assert_eq!(row_size(&a), "-");
    assert_eq!(total(4096, true), "4.0 KiB + unknown");
    assert_eq!(total(4096, false), "4.0 KiB");
    // JSON is unchanged: the size is 0 and nothing says it is unknown.
    let v = serde_json::to_value(&a).unwrap();
    assert_eq!(v["size"], 0);
    assert!(v.get("size_known").is_none(), "{v}");
    a.size_known = true;
    assert_eq!(row_size(&a), fmt_size(0));
}

/// The output of [`report_to`] for one plan of command actions, each
/// with the measured space its result carries.
fn report_text(cmds: &[(&str, &[&str], Option<u64>)]) -> String {
    use crumbs_core::executor::{ActionResult, ExecReport};
    use crumbs_core::safety::Fingerprint;
    let actions: Vec<PlannedAction> = cmds
        .iter()
        .map(|(program, args, _)| PlannedAction {
            action: Action::PrivilegedCommand {
                program: (*program).into(),
                args: args.iter().map(|a| (*a).to_string()).collect(),
            },
            size: 0,
            fingerprint: Fingerprint::default(),
            scope: Scope::System,
            depends_on: None,
            depends_on_plan: None,
            size_known: true,
            reason: String::new(),
        })
        .collect();
    let results = actions
        .iter()
        .zip(cmds)
        .map(|(pa, (_, _, freed))| ActionResult {
            action: pa.action.clone(),
            ok: true,
            error: None,
            history_id: "1-0".into(),
            rights_missing: false,
            freed: *freed,
        })
        .collect();
    let plan = Plan {
        app_id: None,
        kind: Some("clean".into()),
        total_size: 0,
        actions,
        notes: Vec::new(),
    };
    let report = ExecReport {
        batch: "1".into(),
        results,
        freed: 0,
        escalation_failed: false,
        batch_stopped: false,
        escalation_output: Vec::new(),
    };
    let t = crumbs_core::testing::FixtureTree::new();
    let mut out = Vec::new();
    report_to(&mut out, &t.env(), &[(&plan, &report)]).unwrap();
    String::from_utf8(out).unwrap()
}

#[test]
fn measured_command_counts_as_freed() {
    let text = report_text(&[("paccache", &["-rk2"], Some(5 * 1024 * 1024))]);
    assert!(
        text.contains("✓ ran paccache -rk2 (freed 5.0 MiB)"),
        "{text}"
    );
    assert!(text.trim_end().ends_with("freed 5.0 MiB"), "{text}");
    assert!(!text.contains("plus space"), "{text}");
}

#[test]
fn unmeasured_command_still_says_so() {
    let text = report_text(&[
        ("paccache", &["-rk2"], Some(1024 * 1024)),
        ("journalctl", &["--vacuum-size=500M"], None),
    ]);
    assert!(
        text.contains("✓ ran journalctl --vacuum-size=500M\n"),
        "{text}"
    );
    assert!(
        text.trim_end()
            .ends_with("freed 1.0 MiB · plus space freed by journalctl"),
        "{text}"
    );
}

#[test]
fn a_script_argument_shows_as_a_placeholder() {
    let t = crumbs_core::testing::FixtureTree::new();
    let a = Action::PrivilegedCommand {
        program: "sh".into(),
        args: vec![
            "-c".into(),
            "a=1\nb=2\necho $a\n".into(),
            "crumbs-snapshots".into(),
            "3".into(),
        ],
    };
    assert_eq!(
        target(&t.env(), &a),
        "sh -c <script, 3 lines> crumbs-snapshots 3"
    );
}

#[test]
fn only_trash_and_edit_can_be_undone() {
    let p = std::path::PathBuf::from("/x");
    assert!(can_undo(&Action::Trash { path: p.clone() }));
    assert!(can_undo(&Action::EditFile {
        path: p.clone(),
        remove_lines: Vec::new()
    }));
    assert!(!can_undo(&Action::Delete { path: p }));
    assert!(!can_undo(&Action::UserCommand {
        program: "flatpak".into(),
        args: Vec::new()
    }));
}
