use super::*;

#[test]
fn clean_flatpak_runs_command() {
    let t = scan_fixture(FLATPAK_RECORD);
    let args_file = real(&t, "/flatpak-args");
    // Not pre-selected unless the category is named.
    let (code, stdout, _) = run_clean(&t, &["--dry-run"]);
    assert_eq!(code, 0, "{stdout}");
    assert!(!stdout.contains("flatpak uninstall"), "{stdout}");
    let (code, stdout, _) = run_clean(&t, &["flatpak-unused", "--dry-run"]);
    assert_eq!(code, 0, "{stdout}");
    let row = stdout
        .lines()
        .find(|l| l.starts_with("run"))
        .unwrap_or_else(|| panic!("{stdout}"));
    assert!(
        row.contains("—") && row.contains("flatpak uninstall --user --unused --noninteractive -y"),
        "{row}"
    );
    assert!(stdout.contains("will run 1 command"), "{stdout}");
    assert!(!args_file.exists());
    let (code, stdout, stderr) = run_clean(&t, &["flatpak-unused", "--yes"]);
    assert_eq!(code, 0, "{stdout}{stderr}");
    assert_eq!(
        std::fs::read_to_string(&args_file).unwrap(),
        "uninstall --user --unused --noninteractive -y\n"
    );
    assert!(
        stdout.contains("✓ ran flatpak uninstall --user --unused --noninteractive -y"),
        "{stdout}"
    );
    assert_eq!(
        stdout.lines().last(),
        Some("freed space decided by flatpak"),
        "{stdout}"
    );
    // With something trashed too, the command's share is mentioned.
    let (code, stdout, _) = run_clean(&t, &["flatpak-unused", "cache", "--yes"]);
    assert_eq!(code, 0, "{stdout}");
    let last = stdout.lines().last().unwrap();
    assert!(
        last.starts_with("moved ")
            && last.contains(" to the trash · plus space freed by flatpak · undo with: "),
        "{stdout}"
    );
}

#[test]
fn clean_dry_run_changes_nothing() {
    if running_as_root() {
        eprintln!("skipped: temp cleanup is not offered as root");
        return;
    }
    let t = scan_fixture(FLATPAK_RECORD);
    old_temp(&t, "old-build.log");
    let before = tree_state(&t);
    let (code, stdout, _) = run_clean(&t, &["--dry-run"]);
    assert_eq!(code, 0, "{stdout}");
    assert!(stdout.contains("~/.cache/foo"), "{stdout}");
    assert!(stdout.contains("/tmp/old-build.log"), "{stdout}");
    let v = stdout_json(crumbs(&t).args([
        "clean",
        "trash",
        "cache",
        "flatpak-unused",
        "--dry-run",
        "--json",
    ]));
    assert_eq!(v["command"], "clean");
    assert!(v["data"]["report"].is_null());
    assert!(
        !v["data"]["plans"][0]["actions"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(tree_state(&t), before);
}

#[test]
fn clean_non_interactive_without_yes_exit_2() {
    let t = scan_fixture(FLATPAK_OK);
    crumbs(&t)
        .args(["clean", "cache"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains(
            "refusing to act without --yes in a non-interactive session",
        ));
    assert!(real(&t, "/home/user/.cache/foo/blob").exists());
}

#[test]
fn clean_json_requires_yes_or_dry_run() {
    let t = scan_fixture(FLATPAK_OK);
    let v = json_error(&t, &["clean", "--json"], 2);
    assert_eq!(v["command"], "clean");
    let msg = v["errors"][0]["message"].as_str().unwrap();
    assert_eq!(msg, "--json needs --yes or --dry-run");
    assert!(real(&t, "/home/user/.cache/foo/blob").exists());
    let v = stdout_json(crumbs(&t).args(["clean", "cache", "--json", "--yes"]));
    assert_eq!(v["command"], "clean");
    assert_eq!(v["data"]["report"][0]["results"][0]["ok"], true, "{v}");
    assert!(!real(&t, "/home/user/.cache/foo").exists());
}

#[test]
fn clean_scanner_error_refuses_without_force() {
    let t = scan_fixture(FLATPAK_RUNTIME_FAIL);
    let (code, _, stderr) = run_clean(&t, &["--yes"]);
    assert_eq!(code, 1, "{stderr}");
    // Why it failed comes first.
    assert!(stderr.contains("warning: flatpak-unused: "), "{stderr}");
    assert!(stderr.contains("boom"), "{stderr}");
    let v = json_error(&t, &["clean", "--yes", "--json"], 1);
    let errors = v["errors"].as_array().unwrap();
    assert!(
        errors
            .iter()
            .any(|e| e["source"] == "flatpak-unused"
                && e["message"].as_str().unwrap().contains("boom")),
        "{v}"
    );
    assert!(
        stderr.contains(
            "refusing: some sources failed (flatpak-unused); re-run with --force to continue"
        ),
        "{stderr}"
    );
    assert!(real(&t, "/home/user/.cache/foo/blob").exists());
    // A preview still works and warns.
    let (code, stdout, stderr) = run_clean(&t, &["--dry-run"]);
    assert_eq!(code, 0, "{stderr}");
    assert!(stdout.contains("~/.cache/foo"), "{stdout}");
    assert!(stderr.contains("warning: flatpak-unused: "), "{stderr}");
    assert!(stderr.contains("boom"), "{stderr}");
    // An inventory source failure refuses too.
    let t2 = scan_fixture(FLATPAK_FAIL);
    let (code, _, stderr) = run_clean(&t2, &["cache", "--yes"]);
    assert_eq!(code, 1, "{stderr}");
    assert!(stderr.contains("some sources failed (flatpak)"), "{stderr}");
    assert!(real(&t2, "/home/user/.cache/foo/blob").exists());
    let (code, _, stderr) = run_clean(&t, &["--yes", "--force"]);
    assert_eq!(code, 0, "{stderr}");
    assert!(!real(&t, "/home/user/.cache/foo").exists());
}

#[test]
fn clean_restore_round_trip() {
    let t = scan_fixture(FLATPAK_OK);
    let (code, stdout, _) = run_clean(&t, &["cache", "--yes"]);
    assert_eq!(code, 0, "{stdout}");
    assert!(!real(&t, "/home/user/.cache/foo").exists());
    let batch = history_json(&t)["data"]["batches"][0]["batch"]
        .as_str()
        .unwrap()
        .to_string();
    let out = crumbs(&t).args(["restore", &batch]).output().unwrap();
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert_eq!(out.status.code(), Some(0), "{stdout}");
    assert!(stdout.contains("✓ restored ~/.cache/foo"), "{stdout}");
    assert_eq!(
        std::fs::read(real(&t, "/home/user/.cache/foo/blob")).unwrap(),
        vec![1u8; 3000]
    );
    assert!(trash_files(&t).iter().all(|n| n != "foo"));
}

#[test]
fn clean_off_category_needs_naming() {
    let t = scan_fixture(FLATPAK_OK);
    let (code, stdout, stderr) = run_clean(&t, &["--yes"]);
    assert_eq!(code, 0, "{stdout}{stderr}");
    assert!(!real(&t, "/home/user/.cache/foo").exists());
    assert!(real(&t, "/home/user/.cargo/registry/cache/crate.bin").exists());
    let (code, stdout, stderr) = run_clean(&t, &["dev-cache", "--yes"]);
    assert_eq!(code, 0, "{stdout}{stderr}");
    assert!(!real(&t, "/home/user/.cargo/registry/cache").exists());
    // Nothing left to clean in the default categories.
    let (code, stdout, _) = run_clean(&t, &["cache", "thumbnails", "temp", "--yes"]);
    assert_eq!(code, 0, "{stdout}");
    let t3 = FixtureTree::new();
    t3.write_exec("/usr/bin/flatpak", FLATPAK_OK.as_bytes());
    let (code, stdout, _) = run_clean(&t3, &["--yes"]);
    assert_eq!(code, 0, "{stdout}");
    // The opt-in flatpak item is not "held back".
    assert_eq!(stdout.trim(), "nothing to clean");
    // Same JSON shape as an empty plan: `report` is an array.
    let v = stdout_json(crumbs(&t3).args(["clean", "--yes", "--json"]));
    assert_eq!(v["data"]["plans"], serde_json::json!([]));
    assert_eq!(v["data"]["report"], serde_json::json!([]));
}

#[test]
fn clean_keeps_running_apps_items() {
    let t = scan_fixture(FLATPAK_OK);
    t.write("/home/user/.cache/firefox/startupCache/x", &[1u8; 2000]);
    t.write(
        "/home/user/.cache/mozilla/firefox/p.default/cache2/x",
        &[1u8; 2000],
    );
    firefox_running(&t);
    let (code, stdout, stderr) = run_clean(&t, &["browser-cache", "cache", "--yes"]);
    assert_eq!(code, 0, "{stdout}{stderr}");
    assert!(real(&t, "/home/user/.cache/firefox/startupCache/x").exists());
    assert!(real(&t, "/home/user/.cache/mozilla/firefox/p.default/cache2/x").exists());
    assert!(!real(&t, "/home/user/.cache/foo").exists(), "{stdout}");
    assert!(!stdout.contains("~/.cache/firefox"), "{stdout}");
    // Only held-back items left: say why nothing happens.
    let (code, stdout, _) = run_clean(&t, &["browser-cache", "--yes"]);
    assert_eq!(code, 0, "{stdout}");
    assert_eq!(
        stdout.trim(),
        "nothing to clean (1 item held back: Firefox is running)"
    );
}

#[test]
fn clean_preview_sanitises_paths() {
    let t = scan_fixture(FLATPAK_OK);
    t.write("/home/user/.cache/ev\ril\x1b[2Jdir/blob", &[1u8; 100]);
    t.write("/home/user/.local/share/Trash/files/tr\rash\x1b[0m", b"x");
    let (code, stdout, _) = run_clean(&t, &["cache", "trash", "--dry-run"]);
    assert_eq!(code, 0, "{stdout}");
    assert!(
        !stdout.contains('\r') && !stdout.contains('\u{1b}'),
        "{stdout:?}"
    );
    assert!(
        stdout.contains("~/.cache/ev\u{FFFD}il\u{FFFD}[2Jdir"),
        "{stdout:?}"
    );
    assert!(
        stdout.contains("Trash/files/tr\u{FFFD}ash\u{FFFD}[0m"),
        "{stdout:?}"
    );
    // JSON keeps the raw path.
    let v = stdout_json(crumbs(&t).args(["clean", "cache", "--dry-run", "--json"]));
    assert!(v.to_string().contains("\\r"), "{v}");
}

#[test]
fn clean_trash_is_opt_in() {
    let t = scan_fixture(FLATPAK_OK);
    let (code, stdout, stderr) = run_clean(&t, &["--dry-run"]);
    assert_eq!(code, 0, "{stdout}{stderr}");
    assert!(!stdout.contains("Trash/"), "{stdout}");
    assert!(!stdout.contains("emptying the trash"), "{stdout}");
    let (code, stdout, stderr) = run_clean(&t, &["--yes"]);
    assert_eq!(code, 0, "{stdout}{stderr}");
    assert!(!real(&t, "/home/user/.cache/foo").exists());
    assert!(real(&t, "/home/user/.local/share/Trash/files/old.txt").exists());
    assert!(real(&t, "/home/user/.local/share/Trash/info/old.txt.trashinfo").exists());
    // Only the trash left: it is not "held back".
    let (code, stdout, _) = run_clean(&t, &["cache", "thumbnails", "trash", "--dry-run"]);
    assert_eq!(code, 0, "{stdout}");
    assert!(stdout.contains("Trash/files/old.txt"), "{stdout}");
    let (code, stdout, _) = run_clean(&t, &["--yes"]);
    assert_eq!(code, 0, "{stdout}");
    assert!(!stdout.contains("held back"), "{stdout}");
    let (code, stdout, stderr) = run_clean(&t, &["trash", "--yes"]);
    assert_eq!(code, 0, "{stdout}{stderr}");
    assert!(!real(&t, "/home/user/.local/share/Trash/files/old.txt").exists());
}

#[test]
fn clean_system_dry_run_preview() {
    let t = system_fixture("");
    let (code, stdout, stderr) = run_out(crumbs(&t).args(["clean", "--system", "--dry-run"]));
    assert_eq!(code, 0, "{stdout}{stderr}");
    assert!(
        stdout.contains("as root: '/usr/bin/paccache' '-rk2'"),
        "{stdout}"
    );
    assert!(
        stdout.contains("as root: '/usr/bin/journalctl' '--vacuum-size=500M'"),
        "{stdout}"
    );
    assert!(
        stdout.contains("as root: '/usr/bin/systemd-tmpfiles' '--clean'"),
        "{stdout}"
    );
    assert!(
        stdout.contains(&format!("as root: '/usr/bin/rm' '-f' '--' '{CORE}'")),
        "{stdout}"
    );
    assert!(
        stdout.contains("note: these steps run with administrator rights; you will be asked once"),
        "{stdout}"
    );
    // Opt-in categories stay out unless named.
    assert!(!stdout.contains("orph"), "{stdout}");
    assert!(!stdout.contains("flatpak"), "{stdout}");
    for f in ["paccache-args", "journal-args", "tmpfiles-args", "rm-args"] {
        assert!(recorded(&t, f).is_none(), "{f}");
    }
    assert!(real(&t, CORE).exists());
    let v = stdout_json(crumbs(&t).args(["clean", "--system", "--dry-run", "--json"]));
    assert!(v["data"]["report"].is_null(), "{v}");
}
