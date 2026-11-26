use super::*;

#[test]
fn restore_batch_round_trip() {
    let t = fixture(FLATPAK_OK);
    with_leftovers(&t);
    crumbs(&t)
        .args(["uninstall", "Tool", "--yes"])
        .assert()
        .code(0);
    assert!(!real(&t, "/home/user/.config/Tool").exists());
    let batch = history_json(&t)["data"]["batches"][0]["batch"]
        .as_str()
        .unwrap()
        .to_string();
    crumbs(&t)
        .args(["restore", &batch, "--dry-run"])
        .assert()
        .code(0);
    assert!(!real(&t, "/home/user/.config/Tool").exists());
    let out = crumbs(&t).args(["restore", &batch]).output().unwrap();
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert_eq!(out.status.code(), Some(0), "{stdout}");
    assert!(stdout.contains("✓ restored ~/.config/Tool"), "{stdout}");
    assert!(real(&t, "/home/user/.config/Tool/settings.json").exists());
    assert!(real(&t, "/home/user/.cache/Tool/blob").exists());
    assert!(real(&t, "/home/user/Applications/Tool-1.0-x86_64.AppImage").exists());
    assert!(trash_files(&t).is_empty());
}

#[test]
fn restore_occupied_exit_3() {
    let t = fixture(FLATPAK_OK);
    with_leftovers(&t);
    crumbs(&t)
        .args(["uninstall", "Tool", "--yes"])
        .assert()
        .code(0);
    t.write("/home/user/.config/Tool/new.json", b"new");
    let batch = history_json(&t)["data"]["batches"][0]["batch"]
        .as_str()
        .unwrap()
        .to_string();
    let out = crumbs(&t).args(["restore", &batch]).output().unwrap();
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert_eq!(out.status.code(), Some(3), "{stdout}");
    assert!(stdout.contains("✗ ~/.config/Tool: "), "{stdout}");
    assert!(stdout.contains("✓ restored ~/.cache/Tool"), "{stdout}");
    // The occupant is untouched and the trashed copy is still there.
    assert_eq!(
        std::fs::read(real(&t, "/home/user/.config/Tool/new.json")).unwrap(),
        b"new"
    );
    let left = trash_files(&t);
    assert_eq!(left.len(), 1, "{left:?}");
    let kept = real(&t, "/home/user/.local/share/Trash/files")
        .join(&left[0])
        .join("settings.json");
    assert!(kept.exists(), "{left:?}");
}

#[test]
fn restore_path_skips_restored_entry() {
    let t = fixture(FLATPAK_OK);
    let remove = |t: &FixtureTree| {
        crumbs(t)
            .args(["uninstall", "Tool", "--leftovers-only", "--yes"])
            .assert()
            .code(0);
    };
    t.write("/home/user/.config/Tool/settings.json", b"one");
    remove(&t);
    t.write("/home/user/.config/Tool/settings.json", b"two");
    remove(&t);
    assert_eq!(batch_ids(&t).len(), 2);
    let path = "/home/user/.config/Tool";
    let v = stdout_json(crumbs(&t).args(["restore", path, "--json"]));
    let first = &v["data"]["outcomes"][0];
    assert_eq!(first["ok"], true, "{v}");
    assert_eq!(
        std::fs::read(real(&t, "/home/user/.config/Tool/settings.json")).unwrap(),
        b"two"
    );
    // The newest entry is restored; the next restore goes for the older one
    // (which is blocked by the restored directory) instead of repeating it.
    let out = crumbs(&t)
        .args(["restore", path, "--json"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(3));
    let v2: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let second = &v2["data"]["outcomes"][0];
    assert_ne!(second["entry_id"], first["entry_id"], "{v2}");
    assert_ne!(second["error"], "already restored", "{v2}");
    // Nothing left to pick once both were handled: nothing matches.
    std::fs::remove_dir_all(real(&t, "/home/user/.config/Tool")).unwrap();
    crumbs(&t).args(["restore", path]).assert().code(0);
    crumbs(&t).args(["restore", path]).assert().code(2);
}

#[test]
fn restore_delete_only_batch_says_nothing_restorable() {
    let t = fixture(FLATPAK_OK);
    with_leftovers(&t);
    json_error_free(
        &t,
        &["uninstall", "Tool", "--permanent", "--yes", "--json"],
        0,
    );
    let batch = history_json(&t)["data"]["batches"][0]["batch"]
        .as_str()
        .unwrap()
        .to_string();
    let (code, stdout, _) = run_out(crumbs(&t).args(["restore", &batch]));
    assert_eq!(code, 0, "{stdout}");
    assert!(
        stdout.contains("nothing in this batch can be restored"),
        "{stdout}"
    );
    let v = json_error_free(&t, &["restore", &batch, "--json"], 0);
    assert_eq!(v["data"]["outcomes"][0]["ok"], true, "{v}");
}

#[test]
fn restore_unknown_selector_exit_2() {
    let t = fixture(FLATPAK_OK);
    with_leftovers(&t);
    crumbs(&t)
        .args(["restore", "/home/user/.config/nope"])
        .assert()
        .code(2)
        .stdout(predicate::str::contains("no matching history entry"));
    crumbs(&t)
        .args(["restore", "nope", "also-nope", "--dry-run"])
        .assert()
        .code(2);
    let v = json_error_free(&t, &["restore", "nope", "--json"], 2);
    assert_eq!(v["data"]["outcomes"][0]["entry_id"], "");
    // One selector that matches and one that does not is still a partial
    // failure.
    crumbs(&t)
        .args(["uninstall", "Tool", "--yes"])
        .assert()
        .code(0);
    let batch = batch_ids(&t)[0].clone();
    crumbs(&t)
        .args(["restore", &batch, "nope", "--dry-run"])
        .assert()
        .code(3);
}

#[test]
fn restore_mixed_clean_batch_skips_deletes() {
    if running_as_root() {
        eprintln!("skipped: temp cleanup is not offered as root");
        return;
    }
    let t = scan_fixture(FLATPAK_OK);
    old_temp(&t, "old-build.log");
    let (code, stdout, stderr) = run_clean(&t, &["cache", "temp", "--yes"]);
    assert_eq!(code, 0, "{stdout}{stderr}");
    assert!(!real(&t, "/tmp/old-build.log").exists());
    let h = history_json(&t);
    let batch = h["data"]["batches"][0]["batch"]
        .as_str()
        .unwrap()
        .to_string();
    let entries = h["data"]["batches"][0]["entries"].as_array().unwrap();
    let delete_id = entries
        .iter()
        .find(|e| e["action"] == "delete")
        .unwrap_or_else(|| panic!("{h}"))["id"]
        .as_str()
        .unwrap()
        .to_string();
    let out = crumbs(&t).args(["restore", &batch]).output().unwrap();
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert_eq!(out.status.code(), Some(0), "{stdout}");
    assert!(stdout.contains("✓ restored ~/.cache/foo"), "{stdout}");
    assert!(!stdout.contains("old-build.log"), "{stdout}");
    assert!(real(&t, "/home/user/.cache/foo/blob").exists());
    // Named by its id, a delete still says why it cannot come back.
    let out = crumbs(&t).args(["restore", &delete_id]).output().unwrap();
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert_eq!(out.status.code(), Some(3), "{stdout}");
    assert!(
        stdout.contains("permanently deleted; cannot be restored"),
        "{stdout}"
    );
}

// ---- review fixes ----

#[test]
fn dry_run_not_refused_on_source_errors() {
    let t = fixture(FLATPAK_FAIL);
    with_leftovers(&t);
    with_orphans(&t);
    let out = crumbs(&t)
        .args(["uninstall", "Tool", "--dry-run"])
        .output()
        .unwrap();
    let stdout = String::from_utf8(out.stdout).unwrap();
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert_eq!(out.status.code(), Some(0), "{stderr}");
    assert!(stdout.contains("will move 3 items"), "{stdout}");
    assert!(stderr.contains("warning: flatpak:"), "{stderr}");
    assert!(!stderr.contains("refusing"), "{stderr}");
    crumbs(&t)
        .args([
            "orphans",
            "--clean",
            "--select",
            "~/.thunderbird",
            "--dry-run",
        ])
        .assert()
        .code(0)
        .stderr(predicate::str::contains("warning: flatpak:"));
    assert!(real(&t, "/home/user/.thunderbird").exists());
    // Executing still refuses.
    crumbs(&t)
        .args(["orphans", "--clean", "--select", "~/.thunderbird", "--yes"])
        .assert()
        .code(1);
    assert!(real(&t, "/home/user/.thunderbird").exists());
}

#[test]
fn json_errors_print_envelope() {
    let t = fixture(FLATPAK_OK);
    with_leftovers(&t);
    let v = json_error(&t, &["uninstall", "nope", "--json", "--yes"], 2);
    assert_eq!(v["command"], "uninstall");
    assert_eq!(v["data"]["plans"], serde_json::json!([]));
    assert!(v["data"]["report"].is_null());
    let msg = v["errors"].as_array().unwrap().last().unwrap()["message"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(msg.contains("no installed app matches 'nope'"), "{msg}");
    json_error(&t, &["uninstall", "Tool", "--json"], 2);
    json_error(&t, &["uninstall", "T-o-o-l", "--json", "--yes"], 2);
    json_error(&t, &["uninstall", "firefox", "--json", "--yes"], 2);
    json_error(&t, &["orphans", "--clean", "--json", "--yes"], 2);
    let f = fixture(FLATPAK_FAIL);
    with_leftovers(&f);
    json_error(&f, &["uninstall", "Tool", "--json", "--yes"], 1);
    assert!(real(&f, "/home/user/.config/Tool").exists());
}

#[test]
fn json_report_aligned_with_plans() {
    let t = fixture(FLATPAK_OK);
    with_leftovers(&t);
    let v = stdout_json(crumbs(&t).args([
        "uninstall",
        "firefox",
        "Tool",
        "--leftovers-only",
        "--json",
        "--yes",
    ]));
    let plans = v["data"]["plans"].as_array().unwrap();
    let report = v["data"]["report"].as_array().unwrap();
    assert_eq!(plans.len(), 2, "{v}");
    assert_eq!(report.len(), 2, "{v}");
    assert!(plans[0]["actions"].as_array().unwrap().is_empty(), "{v}");
    assert!(report[0].is_null(), "{v}");
    assert_eq!(report[1]["results"].as_array().unwrap().len(), 2, "{v}");
}

#[test]
fn json_config_error_has_envelope() {
    let t = fixture(FLATPAK_OK);
    bad_config(&t);
    let cases: [(&str, &[&str]); 5] = [
        ("apps list", &["apps", "list", "--json"]),
        ("apps show", &["apps", "show", "firefox", "--json"]),
        ("orphans", &["orphans", "--json"]),
        ("restore", &["restore", "x", "--json"]),
        ("doctor", &["doctor", "--json"]),
    ];
    for (command, args) in cases {
        let out = crumbs(&t).args(args).output().unwrap();
        assert_eq!(out.status.code(), Some(1), "{args:?}");
        assert!(!out.stderr.is_empty(), "{args:?}: human text on stderr");
        let v: serde_json::Value = serde_json::from_slice(&out.stdout)
            .unwrap_or_else(|e| panic!("{args:?}: {e}: {:?}", out.stdout));
        assert_eq!(v["schema_version"], 1, "{args:?}");
        assert_eq!(v["command"], command, "{args:?}");
        assert!(v["data"].is_null(), "{args:?}");
        let errors = v["errors"].as_array().unwrap();
        assert_eq!(errors.len(), 1, "{args:?}: {v}");
        assert_eq!(errors[0]["source"], "config", "{args:?}");
        assert!(
            errors[0]["message"]
                .as_str()
                .unwrap()
                .contains("config.toml")
        );
    }
    let v = stdout_json(crumbs(&t).args(["uninstall", "Tool", "--json", "--dry-run"]));
    assert_eq!(v["command"], "uninstall");
    assert!(v["data"].is_null());
    assert_eq!(v["errors"][0]["source"], "config");
    // Without --json nothing goes to stdout.
    let out = crumbs(&t).args(["doctor"]).output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(out.stdout.is_empty());
}
