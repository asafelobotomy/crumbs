use super::*;
use crate::cmd::act::{ActFlags, preview_text};
use crumbs_core::testing::FixtureTree;

fn inky() -> FixtureTree {
    let t = FixtureTree::new();
    t.os_release("manjaro", "arch", "Manjaro Linux");
    t.pacman_pkg(
        "inky",
        "1.0-1",
        Some(1000),
        &[
            "usr/bin/inky",
            "usr/share/applications/",
            "usr/share/applications/inky.desktop",
        ],
    );
    t.write_exec("/usr/bin/inky", b"");
    t.desktop(
        "/usr/share/applications/inky.desktop",
        "[Desktop Entry]\nType=Application\nName=Inky\nExec=/usr/bin/inky\n",
    );
    t.write("home/user/.cache/inky/blob", b"cache");
    t.write("home/user/.config/inky/inky.conf", b"x");
    t
}

fn leftovers_only(app: &str) -> UninstallArgs {
    UninstallArgs {
        apps: vec![app.into()],
        desktop_id: None,
        leftovers_only: true,
        keep_data: false,
        show_low: false,
        desktop: false,
        act: ActFlags {
            dry_run: true,
            json: true,
            ..ActFlags::default()
        },
    }
}

/// What `uninstall inky --leftovers-only` plans in [`inky`]: also
/// pinned for the CLI in `tests/cli.rs`
/// (`uninstall_dry_run_json_plan_matches_builder`).
const INKY_LEFTOVERS: [(&str, &str); 2] = [
    ("trash", "/home/user/.cache/inky"),
    ("trash", "/home/user/.config/inky"),
];

fn actions(plan: &serde_json::Value) -> Vec<(String, String)> {
    let mut v: Vec<(String, String)> = plan["actions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| {
            let kind = a["action"]["type"].as_str().unwrap().to_string();
            let path = a["action"]["path"].as_str().unwrap().to_string();
            (kind, path)
        })
        .collect();
    v.sort();
    v
}

#[test]
fn build_uninstall_matches_cli_plan() {
    let t = inky();
    let env = t.env();
    let args = leftovers_only("inky");
    let built = uninstall(&env, &args, None).unwrap();
    // The CLI's own builder and the data `--dry-run --json` prints.
    let cli = uninstall::build(&env, &args, Pick::Defaults).unwrap();
    let data = crate::cmd::act::dry_run_data(&cli.built.targets);
    let plan = serde_json::to_value(&built.targets[0].plan).unwrap();
    assert_eq!(plan, data["plans"][0]);
    let want: Vec<(String, String)> = INKY_LEFTOVERS
        .iter()
        .map(|(k, p)| (k.to_string(), p.to_string()))
        .collect();
    assert_eq!(actions(&plan), want, "{plan:#}");
    assert_eq!(
        built.cli_command,
        ["crumbs", "uninstall", "inky", "--leftovers-only"]
    );
}

#[test]
fn build_select_rejects_unknown_path() {
    let t = inky();
    let select = [PathBuf::from("/etc/passwd")];
    let Err(refusal) = uninstall(&t.env(), &leftovers_only("inky"), Some(&select)) else {
        panic!("not refused");
    };
    assert_eq!(refusal.exit_code, 2);
    assert!(
        refusal.message.starts_with("not found by crumbs:"),
        "{}",
        refusal.message
    );
}

#[test]
fn build_select_plans_exactly_the_named_findings() {
    let t = inky();
    let select = [PathBuf::from("~/.cache/inky")];
    let built = uninstall(&t.env(), &leftovers_only("inky"), Some(&select)).unwrap();
    let paths: Vec<_> = built.targets[0]
        .plan
        .actions
        .iter()
        .filter_map(|a| a.action.path())
        .collect();
    assert_eq!(paths, [Path::new("/home/user/.cache/inky")]);
    assert_eq!(built.cli_command.last().map(String::as_str), Some("-i"));
}

#[test]
fn preview_text_has_no_escape_codes() {
    let t = inky();
    let env = t.env();
    let built = uninstall(&env, &leftovers_only("inky"), None).unwrap();
    let errors = [ErrorEntry::new("rules", "a note".into())];
    let text = preview_text(&env, &built.targets, &built.after_preview, &errors);
    assert!(text.contains("ACTION"), "{text}");
    assert!(text.ends_with("warning: rules: a note\n"), "{text}");
    assert!(!text.contains('\x1b'), "{text:?}");
}

#[test]
fn execute_confirmed_refuses_an_app_that_was_not_allowed() {
    use crate::cmd::act::{AllowedRunning, Privilege, execute_confirmed};
    let t = inky();
    t.pacman_pkg(
        "blot",
        "1.0-1",
        Some(1000),
        &[
            "usr/bin/blot",
            "usr/share/applications/",
            "usr/share/applications/blot.desktop",
        ],
    );
    t.write_exec("/usr/bin/blot", b"");
    t.desktop(
        "/usr/share/applications/blot.desktop",
        "[Desktop Entry]\nType=Application\nName=Blot\nExec=/usr/bin/blot\n",
    );
    t.write("home/user/.cache/blot/blob", b"cache");
    // The user confirmed Inky, which was running.
    t.symlink("/proc/79/exe", "/usr/bin/inky");
    let env = t.env();
    let mut args = leftovers_only("inky");
    args.apps.push("blot".into());
    let built = uninstall(&env, &args, None).unwrap();
    let inky_id = built.targets[0].app.as_ref().unwrap().id.clone();
    let cfg = Config::default();
    let caches = [
        env.sys("/home/user/.cache/inky"),
        env.sys("/home/user/.cache/blot"),
    ];
    let run = |allowed| {
        execute_confirmed(
            &env,
            &cfg,
            &built.targets,
            Privilege::for_run(&env, false),
            allowed,
        )
    };
    let only_inky = || AllowedRunning::Only(vec![inky_id.clone()]);

    // Blot starts after the confirmation.
    t.symlink("/proc/80/exe", "/usr/bin/blot");
    let refused = run(only_inky());
    assert_eq!(refused.exit_code, 1);
    assert!(refused.reports.is_empty());
    assert_eq!(refused.running, ["Blot is running (pid 80)"]);
    let none = run(AllowedRunning::None);
    assert_eq!(none.exit_code, 1);
    assert_eq!(
        none.running,
        ["Inky is running (pid 79)", "Blot is running (pid 80)"]
    );
    assert!(caches.iter().all(|c| c.exists()), "nothing ran");

    // Blot is gone again: only the allowed Inky runs.
    std::fs::remove_file(t.dir.path().join("proc/80/exe")).unwrap();
    let ran = run(only_inky());
    assert_eq!(ran.exit_code, 0, "{:?}", ran.reports);
    assert_eq!(ran.running, ["Inky is running (pid 79)"]);
    assert_eq!(ran.reports.len(), 2);
    assert!(caches.iter().all(|c| !c.exists()));
}

#[test]
fn execute_confirmed_refuses_root_with_another_users_home() {
    use crate::cmd::act::{AllowedRunning, Privilege, execute_confirmed};
    let t = inky();
    let mut env = t.env();
    let built = uninstall(&env, &leftovers_only("inky"), None).unwrap();
    // Root (as under `sudo -E`) with a HOME owned by the test user.
    env.uid = 0;
    let cfg = Config::default();
    let privilege = Privilege::for_run(&env, false);
    let ran = execute_confirmed(&env, &cfg, &built.targets, privilege, AllowedRunning::Any);
    if crumbs_core::testing::running_as_root() {
        // The fixture home is root's own here: nothing to refuse.
        return;
    }
    assert_eq!(ran.exit_code, 2);
    assert!(ran.reports.is_empty());
    let msg = ran.refusal.unwrap();
    assert!(
        msg.contains("as root") && msg.contains("/home/user"),
        "{msg}"
    );
    assert!(env.sys("/home/user/.cache/inky").exists());
}

#[test]
fn execute_confirmed_runs_with_any_running_app_allowed() {
    use crate::cmd::act::{AllowedRunning, Privilege, execute_confirmed};
    let t = inky();
    t.symlink("/proc/79/exe", "/usr/bin/inky");
    let env = t.env();
    let built = uninstall(&env, &leftovers_only("inky"), None).unwrap();
    let cfg = Config::default();
    let privilege = Privilege::for_run(&env, false);
    let ran = execute_confirmed(&env, &cfg, &built.targets, privilege, AllowedRunning::Any);
    assert_eq!(ran.exit_code, 0, "{:?}", ran.reports);
    assert_eq!(ran.running, ["Inky is running (pid 79)"]);
    assert!(!env.sys("/home/user/.cache/inky").exists());
}
