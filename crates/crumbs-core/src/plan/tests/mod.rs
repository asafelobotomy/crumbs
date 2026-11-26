use super::*;
use crate::leftovers::Confidence;
use crate::testing::FixtureTree;

/// A finding for an existing in-system path, fingerprinted now.
pub(crate) fn finding(env: &Env, path: &str, kind: FindingKind) -> Finding {
    let p = PathBuf::from(path);
    Finding {
        size: disk_usage(env, &p),
        fingerprint: safety::fingerprint(env, &p).unwrap(),
        path: p,
        kind,
        confidence: Confidence::High,
        shared: false,
        reason: format!("reason for {path}"),
        preselected: true,
        scope: Scope::Home,
        mime_lines: Vec::new(),
    }
}

fn kinds(plan: &Plan) -> Vec<&'static str> {
    plan.actions
        .iter()
        .map(|a| match a.action {
            Action::Trash { .. } => "trash",
            Action::Delete { .. } => "delete",
            Action::EditFile { .. } => "edit",
            Action::UserCommand { .. } => "command",
            Action::PrivilegedCommand { .. } => "privileged-command",
            Action::PackageRemove { .. } => "package-remove",
        })
        .collect()
}

fn bitem(
    env: &Env,
    path: &str,
    action: crate::bloat::ItemAction,
    scope: Scope,
) -> crate::bloat::BloatItem {
    let p = PathBuf::from(path);
    crate::bloat::BloatItem {
        size_uncompressed: false,
        category: crate::bloat::Category::Cache,
        size: Some(disk_usage(env, &p).bytes),
        fingerprint: Some(safety::fingerprint(env, &p).unwrap()),
        path: Some(p),
        label: format!("label {path}"),
        rebuildable: true,
        preselected: true,
        note: None,
        action,
        scope,
        extra_paths: Vec::new(),
        app_id: None,
    }
}

fn command_item() -> crate::bloat::BloatItem {
    crate::bloat::BloatItem {
        size_uncompressed: false,
        category: crate::bloat::Category::FlatpakUnused,
        path: None,
        label: "remove unused runtimes".into(),
        size: None,
        rebuildable: false,
        preselected: true,
        note: Some("decided by flatpak".into()),
        action: crate::bloat::ItemAction::Command {
            program: "flatpak".into(),
            args: vec!["uninstall".into(), "--unused".into()],
        },
        scope: Scope::Home,
        fingerprint: None,
        extra_paths: Vec::new(),
        app_id: None,
    }
}

fn trash_action() -> Action {
    Action::Trash {
        path: "/home/user/x".into(),
    }
}

mod appimage_clean;
mod unmanaged_build;
