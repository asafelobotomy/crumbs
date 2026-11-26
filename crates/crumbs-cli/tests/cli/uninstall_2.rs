use super::*;

#[test]
fn uninstall_pacman_executes_batch_and_trashes_leftovers() {
    let t = pacman_fixture(PACMAN_OK);
    let (code, stdout, stderr) = run_out(crumbs(&t).env("CRUMBS_TEST_PRIV_RUNNER", "sh").args([
        "uninstall",
        "firefox",
        "--yes",
    ]));
    assert_eq!(code, 0, "{stdout}{stderr}");
    assert_eq!(
        std::fs::read_to_string(real(&t, "/pacman-args")).unwrap(),
        "-Rn --noconfirm firefox gn libjuice\n"
    );
    assert!(!pkg_present(&t, "firefox-159.0.1-1"));
    assert!(!pkg_present(&t, "gn-0.2385.1-1"));
    assert!(pkg_present(&t, "coreutils-9.5-1"));
    assert!(stdout.contains("✓ removed pacman: firefox"), "{stdout}");
    assert!(stdout.contains("✓ trashed ~/.cache/firefox"), "{stdout}");
    assert!(trash_files(&t).contains(&"firefox".to_string()));
    let h = history_json(&t);
    let text = h.to_string();
    assert!(text.contains("package-remove"), "{h}");
}

#[test]
fn uninstall_pacman_refused_by_simulation_exit_1() {
    let t = pacman_fixture(PACMAN_REFUSE);
    let (code, stdout, stderr) = run_out(crumbs(&t).env("CRUMBS_TEST_PRIV_RUNNER", "sh").args([
        "uninstall",
        "firefox",
        "--yes",
    ]));
    assert_eq!(code, 1, "{stdout}{stderr}");
    assert!(stderr.contains("breaks dependency"), "{stderr}");
    assert!(stderr.contains("failed to prepare transaction"), "{stderr}");
    assert!(!stderr.contains('\u{1b}'), "{stderr:?}");
    assert!(pkg_present(&t, "firefox-159.0.1-1"));
    assert!(real(&t, "/home/user/.cache/firefox/blob").exists());
    let v = json_error(&t, &["uninstall", "firefox", "--json", "--yes"], 1);
    let msg = v["errors"].as_array().unwrap().last().unwrap()["message"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(msg.contains("breaks dependency"), "{msg}");
}

#[test]
fn uninstall_pacman_busy_exit_1() {
    let t = pacman_fixture(PACMAN_OK);
    t.write("/var/lib/pacman/db.lck", b"");
    let (code, stdout, stderr) = run_out(crumbs(&t).env("CRUMBS_TEST_PRIV_RUNNER", "sh").args([
        "uninstall",
        "firefox",
        "--yes",
    ]));
    assert_eq!(code, 1, "{stdout}{stderr}");
    assert!(
        stderr.contains("pacman is busy (database lock present)"),
        "{stderr}"
    );
    assert!(!real(&t, "/pacman-args").exists());
    assert!(real(&t, "/home/user/.cache/firefox/blob").exists());
    json_error(&t, &["uninstall", "firefox", "--json", "--yes"], 1);
}

#[test]
fn uninstall_without_escalation_exit_5_prints_manual_commands() {
    let t = pacman_fixture(PACMAN_OK);
    // No fake runner: a debug build never escalates for real.
    let (code, stdout, stderr) = run_out(crumbs(&t).args(["uninstall", "firefox", "--yes"]));
    assert_eq!(code, 5, "{stdout}{stderr}");
    assert!(stdout.contains("pacman: firefox 159.0.1-1"), "{stdout}");
    assert!(
        stderr.contains("no way to get administrator rights"),
        "{stderr}"
    );
    assert!(stderr.contains(MANUAL_PACMAN), "{stderr}");
    assert!(!real(&t, "/pacman-args").exists());
    assert!(pkg_present(&t, "firefox-159.0.1-1"));
    assert!(real(&t, "/home/user/.cache/firefox/blob").exists());
    assert!(!real(&t, "/home/user/.local/state/crumbs/history.jsonl").exists());
    // A preview needs no escalation.
    crumbs(&t)
        .args(["uninstall", "firefox", "--dry-run"])
        .assert()
        .code(0);
    let v = json_error(&t, &["uninstall", "firefox", "--json", "--yes"], 5);
    assert_eq!(
        v["data"]["manual_commands"],
        serde_json::json!([MANUAL_PACMAN]),
        "{v}"
    );
}

#[test]
fn uninstall_escalation_denied_exit_5_leftovers_untouched() {
    let t = pacman_fixture(PACMAN_OK);
    let (code, stdout, stderr) = run_out(crumbs(&t).env("CRUMBS_TEST_PRIV_RUNNER", "deny").args([
        "uninstall",
        "firefox",
        "--yes",
    ]));
    assert_eq!(code, 5, "{stdout}{stderr}");
    assert!(
        stderr
            .contains("administrator rights were not granted; nothing that needs them was changed"),
        "{stderr}"
    );
    assert!(stderr.contains(MANUAL_PACMAN), "{stderr}");
    assert!(stdout.contains("✗ pacman: firefox"), "{stdout}");
    assert!(pkg_present(&t, "firefox-159.0.1-1"));
    assert!(real(&t, "/home/user/.cache/firefox/blob").exists());
    assert!(trash_files(&t).is_empty());
    // The attempt is recorded.
    let h = history_json(&t);
    assert!(h.to_string().contains("package-remove"), "{h}");
    let out = crumbs(&t)
        .env("CRUMBS_TEST_PRIV_RUNNER", "deny")
        .args(["uninstall", "firefox", "--yes", "--json"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(5));
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(
        v["data"]["manual_commands"],
        serde_json::json!([MANUAL_PACMAN]),
        "{v}"
    );
    assert_eq!(v["data"]["report"][0]["escalation_failed"], true, "{v}");
    assert!(
        v["errors"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["source"] == "cli"),
        "{v}"
    );
}

#[test]
fn uninstall_flatpak_user_runs_command() {
    let t = fixture(FLATPAK_APPS_RECORD);
    t.write("/home/user/.var/app/app.xemu.xemu/config/x", b"x");
    let (code, stdout, stderr) =
        run_out(crumbs(&t).args(["uninstall", "app.xemu.xemu", "--dry-run"]));
    assert_eq!(code, 0, "{stdout}{stderr}");
    assert!(
        stdout.contains("flatpak uninstall --user -y --noninteractive app.xemu.xemu"),
        "{stdout}"
    );
    assert!(!stdout.contains("administrator rights"), "{stdout}");
    // No privileged action: no escalation is needed.
    let (code, stdout, stderr) = run_out(crumbs(&t).args(["uninstall", "app.xemu.xemu", "--yes"]));
    assert_eq!(code, 0, "{stdout}{stderr}");
    assert_eq!(
        std::fs::read_to_string(real(&t, "/flatpak-args")).unwrap(),
        "uninstall --user -y --noninteractive app.xemu.xemu\n"
    );
    assert!(!real(&t, "/home/user/.var/app/app.xemu.xemu").exists());
    assert!(trash_files(&t).contains(&"app.xemu.xemu".to_string()));
}

#[test]
fn uninstall_flatpak_system_uses_batch() {
    let t = fixture(FLATPAK_APPS_RECORD);
    t.write("/home/user/.var/app/com.dosbox_x.DOSBox-X/config/x", b"x");
    let manual = "sudo '/usr/bin/flatpak' 'uninstall' '--system' '-y' '--noninteractive' 'com.dosbox_x.DOSBox-X'";
    let (code, _, stderr) = run_out(crumbs(&t).args(["uninstall", "DOSBox-X", "--yes"]));
    assert_eq!(code, 5, "{stderr}");
    assert!(stderr.contains(manual), "{stderr}");
    assert!(!real(&t, "/flatpak-args").exists());
    let (code, stdout, stderr) = run_out(crumbs(&t).env("CRUMBS_TEST_PRIV_RUNNER", "sh").args([
        "uninstall",
        "DOSBox-X",
        "--yes",
    ]));
    assert_eq!(code, 0, "{stdout}{stderr}");
    assert!(
        stdout.contains("note: these steps run with administrator rights; you will be asked once"),
        "{stdout}"
    );
    assert_eq!(
        std::fs::read_to_string(real(&t, "/flatpak-args")).unwrap(),
        "uninstall --system -y --noninteractive com.dosbox_x.DOSBox-X\n"
    );
    assert!(!real(&t, "/home/user/.var/app/com.dosbox_x.DOSBox-X").exists());
}

#[test]
fn uninstall_two_apps_asks_once() {
    let t = two_pacman_apps();
    let (code, stdout, stderr) = run_out(crumbs(&t).env("CRUMBS_TEST_PRIV_RUNNER", "sh").args([
        "uninstall",
        "firefox",
        "kate",
        "--yes",
    ]));
    assert_eq!(code, 0, "{stdout}{stderr}");
    assert_eq!(
        stderr.matches("crumbs-test: privileged batch").count(),
        1,
        "{stderr}"
    );
    assert_eq!(
        std::fs::read_to_string(real(&t, "/pacman-args")).unwrap(),
        "-Rn --noconfirm firefox gn libjuice\n-Rn --noconfirm kate\n"
    );
    assert!(stdout.contains("✓ removed pacman: kate"), "{stdout}");
    assert!(!real(&t, "/home/user/.cache/kate").exists());
    // Each app keeps its own batch.
    let h = history_json(&t);
    assert_eq!(h["data"]["batches"].as_array().unwrap().len(), 2, "{h}");
}

#[test]
fn uninstall_two_apps_denied_fails_both() {
    let t = two_pacman_apps();
    let (code, stdout, stderr) = run_out(crumbs(&t).env("CRUMBS_TEST_PRIV_RUNNER", "deny").args([
        "uninstall",
        "firefox",
        "kate",
        "--yes",
    ]));
    assert_eq!(code, 5, "{stdout}{stderr}");
    for app in ["firefox", "kate"] {
        assert!(
            stdout.contains(&format!(
                "✗ pacman: {app}: privilege escalation was cancelled or denied"
            )),
            "{stdout}"
        );
    }
    assert!(!stdout.contains("no way to get"), "{stdout}");
    assert!(
        stderr.contains("sudo '/usr/bin/pacman' '-Rn' '--noconfirm' 'kate'"),
        "{stderr}"
    );
    assert!(real(&t, "/home/user/.cache/kate/blob").exists());
}

#[test]
fn uninstall_lists_system_leftovers_not_preselected() {
    let t = inky_pkg_fixture();
    let (code, stdout, stderr) =
        run_out(sh_runner(&t).args(["uninstall", "inky", "--leftovers-only", "--yes"]));
    assert_eq!(code, 0, "{stdout}{stderr}");
    // `--yes` alone never selects a system leftover.
    assert!(real(&t, "/opt/inky/bin/inky").exists());
    assert!(real(&t, "/etc/inky/inky.conf").exists());
    assert!(!real(&t, "/rm-args").exists());
    assert!(!stdout.contains("as root"), "{stdout}");
}

#[test]
fn uninstall_preview_notes_unselected_system_leftovers() {
    let t = inky_pkg_fixture();
    let (code, stdout, stderr) =
        run_out(crumbs(&t).args(["uninstall", "inky", "--leftovers-only", "--dry-run"]));
    assert_eq!(code, 0, "{stdout}{stderr}");
    assert!(
        stdout.contains("note: 2 system leftovers not selected (pick with -i)\n"),
        "{stdout}"
    );
    // One left: singular.
    let t = inky_unmanaged_fixture();
    let (code, stdout, stderr) =
        run_out(crumbs(&t).args(["uninstall", "unmanaged:inky.desktop", "--dry-run"]));
    assert_eq!(code, 0, "{stdout}{stderr}");
    assert!(
        stdout.contains("note: 1 system leftover not selected (pick with -i)\n"),
        "{stdout}"
    );
    // JSON carries no such note.
    let v = stdout_json(crumbs(&t).args([
        "uninstall",
        "unmanaged:inky.desktop",
        "--dry-run",
        "--json",
    ]));
    assert!(!v.to_string().contains("not selected"), "{v}");
    // Nothing unselected, no note.
    let t = fixture(FLATPAK_OK);
    with_leftovers(&t);
    let (_, stdout, _) = run_out(crumbs(&t).args(["uninstall", "Tool", "--dry-run"]));
    assert!(!stdout.contains("not selected"), "{stdout}");
}

#[test]
fn uninstall_unmanaged_removes_opt_dir_as_root() {
    let t = inky_unmanaged_fixture();
    let (code, stdout, stderr) =
        run_out(crumbs(&t).args(["uninstall", "unmanaged:inky.desktop", "--dry-run"]));
    assert_eq!(code, 0, "{stdout}{stderr}");
    assert!(stdout.contains("delete (as root)"), "{stdout}");
    assert!(stdout.contains("as root: "), "{stdout}");
    assert!(real(&t, "/opt/inky/inky").exists());
    let (code, stdout, stderr) =
        run_out(sh_runner(&t).args(["uninstall", "unmanaged:inky.desktop", "--yes"]));
    assert_eq!(code, 0, "{stdout}{stderr}");
    assert!(
        stdout.contains(
            "note: system files are deleted permanently with administrator rights and cannot be restored"
        ),
        "{stdout}"
    );
    assert!(stdout.contains("delete (as root)"), "{stdout}");
    assert!(!real(&t, "/opt/inky").exists());
    // The unselected leftover in /etc stays.
    assert!(real(&t, "/etc/inky/inky.conf").exists());
    let rm = std::fs::read_to_string(real(&t, "/rm-args")).unwrap();
    assert_eq!(rm.lines().count(), 1, "one batch: {rm}");
    let h = history_json(&t);
    assert!(h.to_string().contains("system-tree"), "{h}");
}
