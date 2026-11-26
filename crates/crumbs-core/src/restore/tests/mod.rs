use super::*;
use crate::config::Config;
use crate::executor::{ExecReport, execute};
use crate::history::{self, HistoryAction, HistoryEntry, read_history};
use crate::plan::{Action, Plan, PlannedAction};
use crate::safety::fingerprint;
use crate::testing::FixtureTree;
use std::fs;
use std::path::Path;

fn run(env: &Env, actions: Vec<Action>) -> ExecReport {
    let actions: Vec<PlannedAction> = actions
        .into_iter()
        .map(|action| PlannedAction {
            group: None,
            size: 1,
            fingerprint: fingerprint(env, action.path().unwrap()).unwrap(),
            scope: crate::safety::Scope::Home,
            depends_on: None,
            depends_on_plan: None,
            size_known: true,
            reason: "test".into(),
            action,
        })
        .collect();
    let plan = Plan {
        app_id: Some("pacman:foo".into()),
        kind: None,
        total_size: actions.len() as u64,
        actions,
        notes: Vec::new(),
    };
    let report = execute(env, &Config::default(), &plan);
    assert!(report.results.iter().all(|r| r.ok), "{report:?}");
    report
}

fn trash(p: &str) -> Action {
    Action::Trash { path: p.into() }
}

fn sel(s: &str) -> Vec<String> {
    vec![s.to_string()]
}

fn restore_entries(env: &Env) -> Vec<HistoryEntry> {
    read_history(env)
        .unwrap()
        .0
        .into_iter()
        .filter(|e| e.action == HistoryAction::Restore)
        .collect()
}

fn edit_mime(env: &Env, t: &FixtureTree, original: &[u8]) -> ExecReport {
    t.write("home/user/.config/mimeapps.list", original);
    run(
        env,
        vec![Action::EditFile {
            path: "/home/user/.config/mimeapps.list".into(),
            remove_lines: vec!["text/plain=foo.desktop".into()],
        }],
    )
}

/// A genuine trash entry rewritten to point into `root` (in-system).
fn entry_in(env: &Env, root: &str) -> HistoryEntry {
    let mut e = read_history(env).unwrap().0[0].clone();
    let tr = e.trashed.as_mut().unwrap();
    let name = tr.files_path.file_name().unwrap().to_os_string();
    let mut info = name.clone();
    info.push(".trashinfo");
    tr.files_path = Path::new(root).join("files").join(&name);
    tr.info_path = Path::new(root).join("info").join(info);
    e
}

fn mode(t: &FixtureTree, rel: &str, m: u32) {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(t.dir.path().join(rel), fs::Permissions::from_mode(m)).unwrap();
}

fn topdir_fixture() -> (FixtureTree, Env) {
    let t = FixtureTree::new();
    t.write("home/user/.config/a/x", b"A");
    let env = t.env();
    run(&env, vec![trash("/home/user/.config/a")]);
    let uid = env.uid;
    t.mkdir(&format!("home/.Trash-{uid}/files"));
    t.mkdir(&format!("home/.Trash-{uid}/info"));
    mode(&t, &format!("home/.Trash-{uid}"), 0o700);
    (t, env)
}

mod restore;
mod restore_topdir;
mod shared_home;
