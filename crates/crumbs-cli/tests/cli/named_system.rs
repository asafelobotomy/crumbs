use super::*;

#[test]
fn named_category_of_the_other_kind_is_usage_error() {
    let t = system_fixture("");
    let cases: [(&[&str], &str); 4] = [
        (&["clean", "--user", "journal", "--dry-run"], "--user"),
        (&["clean", "--system", "cache", "--dry-run"], "--system"),
        (&["scan", "--user", "--category", "journal"], "--user"),
        (&["scan", "--system", "--category", "cache"], "--system"),
    ];
    for (args, flag) in cases {
        let (code, _, stderr) = run_out(crumbs(&t).args(args));
        assert_eq!(code, 2, "{args:?}: {stderr}");
        assert!(
            stderr.contains("category") && stderr.contains(&format!("cannot be used with {flag}")),
            "{args:?}: {stderr}"
        );
    }
    // The same kind is fine.
    crumbs(&t)
        .args(["clean", "--system", "journal", "--dry-run"])
        .assert()
        .code(0);
}

#[test]
fn system_delete_denied_exit_5_lists_rm_rf() {
    let t = inky_unmanaged_fixture();
    let (code, stdout, stderr) = run_out(crumbs(&t).env("CRUMBS_TEST_PRIV_RUNNER", "deny").args([
        "uninstall",
        "unmanaged:inky.desktop",
        "--yes",
    ]));
    assert_eq!(code, 5, "{stdout}{stderr}");
    assert!(real(&t, "/opt/inky/inky").exists());
    assert!(
        stderr.contains(
            "  sudo '/usr/bin/rm' '-rf' '--one-file-system' '--' '/opt/inky'\n\
             check that nothing is mounted inside these paths before running the rm commands\n"
        ),
        "{stderr}"
    );
    let v = json_error(
        &t,
        &["uninstall", "unmanaged:inky.desktop", "--yes", "--json"],
        5,
    );
    assert_eq!(
        v["data"]["manual_commands"],
        serde_json::json!(["sudo '/usr/bin/rm' '-rf' '--one-file-system' '--' '/opt/inky'"]),
        "{v}"
    );
}

#[test]
fn version_flag_prints_name_and_version() {
    Command::cargo_bin("crumbs")
        .unwrap()
        .arg("--version")
        .assert()
        .success()
        .stdout(format!("crumbs {}\n", env!("CARGO_PKG_VERSION")));
}

#[test]
fn config_set_then_show_json() {
    let t = fixture(FLATPAK_OK);
    let v = stdout_json(crumbs(&t).args(["config", "set", "temp_age_days", "3", "--json"]));
    assert_eq!(v["command"], "config set");
    assert_eq!(v["data"]["changed"], true);
    assert_eq!(v["data"]["values"]["temp_age_days"], 3);
    let v = stdout_json(crumbs(&t).args(["config", "add", "protect", "~/Games", "--json"]));
    assert_eq!(v["data"]["changed"], true);
    let v = stdout_json(crumbs(&t).args(["config", "show", "--json"]));
    assert_eq!(v["command"], "config show");
    assert!(v["data"].get("changed").is_none());
    assert_eq!(v["data"]["values"]["temp_age_days"], 3);
    assert_eq!(v["data"]["values"]["protect"][0], "~/Games");
    assert_eq!(v["data"]["keys"].as_array().unwrap().len(), 12);
    crumbs(&t)
        .args(["config", "reset", "all"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Saved."));
}

#[test]
fn config_refuses_bad_value() {
    let t = fixture(FLATPAK_OK);
    let out = crumbs(&t)
        .args(["config", "set", "temp_age_days", "0"])
        .output()
        .unwrap();
    assert_ne!(out.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&out.stderr).contains("at least 1"));
}
