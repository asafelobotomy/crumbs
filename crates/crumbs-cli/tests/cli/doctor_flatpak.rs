use super::*;

#[test]
fn doctor_shows_privileged_availability() {
    if running_as_root() {
        eprintln!("skipped: escalation detection reports running as root");
        return;
    }
    let t = fixture(FLATPAK_OK);
    crumbs(&t)
        .arg("doctor")
        .assert()
        .code(0)
        .stdout(predicate::str::contains("privileged cleanup: unavailable"));
    // Only detected: doctor never runs the escalation program.
    t.write_exec("/usr/bin/run0", b"#!/bin/sh\nexit 99\n");
    t.write_exec(
        "/usr/bin/systemctl",
        b"#!/bin/sh\necho 'systemd 261 (261.3-1-manjaro)'\n",
    );
    crumbs(&t)
        .arg("doctor")
        .assert()
        .code(0)
        .stdout(predicate::str::contains(
            "privileged cleanup: available via run0",
        ));
}

#[test]
fn doctor_sudo_without_terminal_is_unavailable() {
    if running_as_root() {
        eprintln!("skipped: escalation detection reports running as root");
        return;
    }
    let t = fixture(FLATPAK_OK);
    t.write_exec("/usr/bin/sudo", b"#!/bin/sh\nexit 99\n");
    crumbs(&t)
        .arg("doctor")
        .assert()
        .code(0)
        .stdout(predicate::str::contains("needs a terminal"))
        .stdout(predicate::str::contains("privileged cleanup: unavailable"));
}

#[test]
fn flatpak_failure_exit_3() {
    let t = fixture(FLATPAK_FAIL);
    crumbs(&t)
        .args(["apps", "list"])
        .assert()
        .code(3)
        .stdout(predicate::str::contains("pacman:firefox"))
        .stderr(predicate::str::contains("warning: flatpak:"));
}

#[test]
fn flatpak_failure_json_has_errors_and_no_stderr_warning() {
    let t = fixture(FLATPAK_FAIL);
    let out = crumbs(&t)
        .args(["apps", "list", "--json"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(3));
    assert!(out.stderr.is_empty());
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["errors"][0]["source"], "flatpak");
}

#[test]
fn flatpak_unused_dry_run_only_asks_flatpak() {
    let t = scan_fixture(FLATPAK_PROBED);
    let (code, stdout, stderr) = run_clean(&t, &["flatpak-unused", "--dry-run"]);
    assert_eq!(code, 0, "{stdout}{stderr}");
    assert!(stdout.contains("org.gnome.Platform//45"), "{stdout}");
    // The scan asked in the declining form only; nothing was removed.
    let args = std::fs::read_to_string(real(&t, "/flatpak-args")).unwrap();
    assert_eq!(args, "uninstall --user --unused\n");
}

#[test]
fn orphans_lists_medium() {
    let t = fixture(FLATPAK_OK);
    with_orphans(&t);
    let text = snapshot_text(&t, &["orphans"]);
    let text = without_dir_blocks(
        &t,
        text,
        &[
            ("~/.thunderbird", 4 * 1024, 1),
            ("~/.var/app/org.gone.App", 0, 1),
            ("2 orphans", 4 * 1024, 2),
        ],
    );
    insta::assert_snapshot!(text);
}

#[test]
fn orphans_show_low_includes_low() {
    let t = fixture(FLATPAK_OK);
    with_orphans(&t);
    let plain = snapshot_text(&t, &["orphans"]);
    assert!(!plain.contains("zzquux-thing"), "{plain}");
    let all = snapshot_text(&t, &["orphans", "--show-low"]);
    assert!(all.contains("zzquux-thing"), "{all}");
    assert!(
        all.lines()
            .any(|l| l.starts_with("low") && l.contains("zzquux-thing")),
        "{all}"
    );
}

#[test]
fn orphans_clean_select() {
    let t = fixture(FLATPAK_OK);
    with_orphans(&t);
    // Never cleans implicitly.
    crumbs(&t)
        .args(["orphans", "--clean", "--yes"])
        .assert()
        .code(2);
    assert!(real(&t, "/home/user/.thunderbird").exists());
    let out = crumbs(&t)
        .args(["orphans", "--clean", "--select", "~/.thunderbird", "--yes"])
        .output()
        .unwrap();
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert_eq!(out.status.code(), Some(0), "{stdout}");
    assert!(stdout.contains("✓ trashed ~/.thunderbird"), "{stdout}");
    assert!(!real(&t, "/home/user/.thunderbird").exists());
    assert!(real(&t, "/home/user/.var/app/org.gone.App").exists());
    assert_eq!(trash_files(&t), [".thunderbird"]);
    // A path that is not an orphan is refused.
    crumbs(&t)
        .args([
            "orphans",
            "--clean",
            "--select",
            "/home/user/.bashrc",
            "--yes",
        ])
        .assert()
        .code(2);
}

#[test]
fn orphans_system_lists_unowned_opt_dirs() {
    let t = old_app_fixture();
    let (code, stdout, stderr) = run_out(crumbs(&t).args(["orphans", "--system"]));
    assert_eq!(code, 0, "{stdout}{stderr}");
    assert!(stdout.contains("/opt/old-app"), "{stdout}");
    assert!(stdout.contains("system-data"), "{stdout}");
    assert!(!stdout.contains("/opt/inky"), "{stdout}");
    let (_, stdout, _) = run_out(crumbs(&t).args(["orphans"]));
    assert!(!stdout.contains("/opt/old-app"), "{stdout}");
    let v = stdout_json(crumbs(&t).args(["orphans", "--system", "--json"]));
    let o = &v["data"]["orphans"][0];
    assert_eq!(o["path"], "/opt/old-app", "{v}");
    assert_eq!(o["scope"], "system-tree", "{v}");
    // Home findings carry no scope.
    let v = stdout_json(crumbs(&t).args(["orphans", "--json"]));
    assert!(
        v["data"]["orphans"]
            .as_array()
            .unwrap()
            .iter()
            .all(|o| o.get("scope").is_none()),
        "{v}"
    );
}

#[test]
fn orphans_system_never_lists_etc() {
    let t = old_app_fixture();
    let (_, stdout, _) = run_out(crumbs(&t).args(["orphans", "--system", "--show-low"]));
    assert!(!stdout.contains("/etc/"), "{stdout}");
    let (code, _, stderr) = run_out(sh_runner(&t).args([
        "orphans",
        "--system",
        "--clean",
        "--select",
        "/etc/old-app",
        "--yes",
    ]));
    assert_eq!(code, 2, "{stderr}");
    assert!(stderr.contains("not an orphan"), "{stderr}");
    assert!(real(&t, "/etc/old-app/x.conf").exists());
}

#[test]
fn orphans_system_select_deletes_as_root() {
    let t = old_app_fixture();
    // `--yes` alone selects nothing.
    let (code, _, _) = run_out(sh_runner(&t).args(["orphans", "--system", "--clean", "--yes"]));
    assert_eq!(code, 2);
    assert!(real(&t, "/opt/old-app/bin/x").exists());
    // A user orphan path is not a system orphan.
    t.write("/home/user/.config/zzzgone/a", b"x");
    let (code, _, stderr) = run_out(sh_runner(&t).args([
        "orphans",
        "--system",
        "--clean",
        "--select",
        "~/.config/zzzgone",
        "--show-low",
        "--yes",
    ]));
    assert_eq!(code, 2, "{stderr}");
    assert!(real(&t, "/home/user/.config/zzzgone/a").exists());
    let (code, stdout, stderr) = run_out(sh_runner(&t).args([
        "orphans",
        "--system",
        "--clean",
        "--select",
        "/opt/old-app",
        "--yes",
    ]));
    assert_eq!(code, 0, "{stdout}{stderr}");
    assert!(!real(&t, "/opt/old-app").exists());
    assert!(real(&t, "/opt/inky/inky").exists());
    assert!(stdout.contains("delete (as root)"), "{stdout}");
}

#[test]
fn orphans_user_and_system_conflict_exit_2() {
    let t = old_app_fixture();
    let (code, _, stderr) = run_out(crumbs(&t).args(["orphans", "--user", "--system"]));
    assert_eq!(code, 2);
    assert!(stderr.contains("use either --user or --system"), "{stderr}");
    json_error(&t, &["orphans", "--user", "--system", "--json"], 2);
}

#[test]
fn bad_user_rule_warns_but_succeeds() {
    let t = fixture(FLATPAK_OK);
    t.write("/home/user/.config/crumbs/rules.d/bad.toml", b"[[rule]\n");
    let out = crumbs(&t).args(["orphans"]).output().unwrap();
    assert_eq!(out.status.code(), Some(0));
    assert!(
        String::from_utf8(out.stderr)
            .unwrap()
            .contains("warning: rules: ")
    );
    let v = stdout_json(crumbs(&t).args(["orphans", "--json"]));
    assert_eq!(v["errors"][0]["source"], "rules");
}

#[test]
fn uninstall_appimage_trashes_file_and_leftovers() {
    let t = fixture(FLATPAK_OK);
    gutcheck(&t);
    let out = crumbs(&t)
        .args(["uninstall", "GutCheck", "--yes"])
        .output()
        .unwrap();
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert_eq!(out.status.code(), Some(0), "{stdout}");
    assert!(!real(&t, "/home/user/.config/GutCheck").exists());
    assert!(
        !real(
            &t,
            "/home/user/Applications/GutCheck-0.19.0-x86_64.AppImage"
        )
        .exists()
    );
    let trash = trash_files(&t);
    assert!(trash.contains(&"GutCheck".to_string()), "{trash:?}");
    assert!(
        trash.contains(&"GutCheck-0.19.0-x86_64.AppImage".to_string()),
        "{trash:?}"
    );
    assert!(stdout.contains("✓ trashed ~/.config/GutCheck"), "{stdout}");
    assert!(stdout.contains("will move 3 items"), "{stdout}");
    let h = history_json(&t);
    let batches = h["data"]["batches"].as_array().unwrap();
    assert_eq!(batches.len(), 1, "{h}");
    let batch = batches[0]["batch"].as_str().unwrap();
    assert!(
        stdout.contains(&format!("· undo with: crumbs restore {batch}")),
        "{stdout}"
    );
    assert_eq!(batches[0]["items"], 3);
}

#[test]
fn uninstall_dry_run_changes_nothing() {
    let t = fixture(FLATPAK_OK);
    gutcheck(&t);
    let out = crumbs(&t)
        .args(["uninstall", "GutCheck", "--dry-run"])
        .output()
        .unwrap();
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert_eq!(out.status.code(), Some(0), "{stdout}");
    assert!(stdout.contains("will move 3 items"), "{stdout}");
    assert!(stdout.contains("~/.config/GutCheck"), "{stdout}");
    assert!(real(&t, "/home/user/.config/GutCheck").exists());
    assert!(
        real(
            &t,
            "/home/user/Applications/GutCheck-0.19.0-x86_64.AppImage"
        )
        .exists()
    );
    assert!(trash_files(&t).is_empty());
    assert!(!real(&t, "/home/user/.local/state/crumbs/history.jsonl").exists());
}

#[test]
fn uninstall_leftovers_only_pacman_keeps_package() {
    let t = fixture(FLATPAK_OK);
    t.write("/home/user/.cache/firefox/blob", b"x");
    let out = crumbs(&t)
        .args(["uninstall", "firefox", "--leftovers-only", "--yes"])
        .output()
        .unwrap();
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert_eq!(out.status.code(), Some(0), "{stdout}");
    assert!(stdout.contains("leftovers only"), "{stdout}");
    assert!(!real(&t, "/home/user/.cache/firefox").exists());
    assert!(trash_files(&t).contains(&"firefox".to_string()));
    assert!(real(&t, "/usr/bin/firefox").exists());
    assert!(real(&t, "/var/lib/pacman/local/firefox-159.0.1-1/desc").exists());
}
