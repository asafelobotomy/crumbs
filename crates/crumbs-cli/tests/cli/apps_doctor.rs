use super::*;

#[test]
fn apps_list_snapshot() {
    let t = fixture(FLATPAK_OK);
    let out = crumbs(&t).args(["apps", "list"]).output().unwrap();
    assert_eq!(out.status.code(), Some(0));
    let text = String::from_utf8(out.stdout).unwrap();
    let root = regex_escape(&t.dir.path().to_string_lossy());
    insta::with_settings!({ filters => vec![(root.as_str(), "[ROOT]")] }, {
        insta::assert_snapshot!(text);
    });
}

#[test]
fn apps_list_json_envelope() {
    let t = fixture(FLATPAK_OK);
    let v = stdout_json(crumbs(&t).args(["apps", "list", "--json"]));
    assert_eq!(v["schema_version"], 1);
    assert_eq!(v["command"], "apps list");
    assert_eq!(v["data"]["apps"].as_array().unwrap().len(), 6);
    assert!(v["data"]["statuses"].is_array());
    assert!(v["errors"].as_array().unwrap().is_empty());
}

#[test]
fn apps_list_source_filter() {
    let t = fixture(FLATPAK_OK);
    let out = crumbs(&t)
        .args(["apps", "list", "--source", "appimage"])
        .output()
        .unwrap();
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(text.contains("1 apps"), "{text}");
    assert_eq!(text.matches("appimage:").count(), 1, "{text}");
    assert!(!text.contains("pacman:"));
}

#[test]
fn apps_show_found() {
    let t = fixture(FLATPAK_OK);
    crumbs(&t)
        .args(["apps", "show", "firefox"])
        .assert()
        .code(0)
        .stdout(predicate::str::contains("pacman:firefox"));
}

#[test]
fn apps_show_not_found_exit_2() {
    let t = fixture(FLATPAK_OK);
    crumbs(&t)
        .args(["apps", "show", "nope"])
        .assert()
        .code(2)
        .stdout("")
        .stderr(predicate::str::contains(
            "error: no installed app matches 'nope'",
        ));
}

#[test]
fn apps_show_ambiguous_exit_2() {
    let t = fixture("#!/bin/sh\nprintf 'org.mozilla.firefox\\tFirefox\\t1.0\\tsystem\\t1 MB\\n'\n");
    t.appimage("/home/user/Applications/Firefox-1.0-x86_64.AppImage");
    crumbs(&t)
        .args(["apps", "show", "FIREFOX"])
        .assert()
        .code(2)
        .stdout("")
        .stderr(
            predicate::str::contains("'FIREFOX' matches several apps:")
                .and(predicate::str::contains("pacman:firefox"))
                .and(predicate::str::contains(
                    "flatpak:system/org.mozilla.firefox",
                ))
                .and(predicate::str::contains(
                    "appimage:~/Applications/Firefox-1.0-x86_64.AppImage",
                )),
        );
}

#[test]
fn apps_show_same_flatpak_in_both_installations_is_ambiguous() {
    let t = fixture(
        "#!/bin/sh\nprintf 'org.x.Y\\tY\\t1.0\\tuser\\t1 MB\\norg.x.Y\\tY\\t1.0\\tsystem\\t1 MB\\n'\n",
    );
    crumbs(&t)
        .args(["apps", "show", "org.x.Y"])
        .assert()
        .code(2)
        .stdout("")
        .stderr(
            predicate::str::contains("'org.x.Y' matches several apps:")
                .and(predicate::str::contains("flatpak:user/org.x.Y"))
                .and(predicate::str::contains("flatpak:system/org.x.Y")),
        );
}

#[test]
fn apps_show_not_found_json_envelope() {
    let t = fixture(FLATPAK_OK);
    let out = crumbs(&t)
        .args(["apps", "show", "nope", "--json"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["command"], "apps show");
    assert!(v["data"]["app"].is_null());
    assert_eq!(v["data"]["query"], "nope");
    assert_eq!(v["data"]["candidates"], serde_json::json!([]));
    assert!(
        String::from_utf8(out.stderr)
            .unwrap()
            .contains("no installed app matches 'nope'")
    );
}

#[test]
fn apps_show_ambiguous_json_envelope() {
    let t = fixture("#!/bin/sh\nprintf 'org.mozilla.firefox\\tFirefox\\t1.0\\tsystem\\t1 MB\\n'\n");
    t.appimage("/home/user/Applications/Firefox-1.0-x86_64.AppImage");
    let out = crumbs(&t)
        .args(["apps", "show", "FIREFOX", "--json"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(v["data"]["app"].is_null());
    assert_eq!(v["data"]["candidates"].as_array().unwrap().len(), 3);
}

#[test]
fn apps_list_pipe_closed_exit_0() {
    let t = fixture(FLATPAK_OK);
    let bin = assert_cmd::cargo::cargo_bin!("crumbs");
    let mut child = std::process::Command::new(bin)
        .env("CRUMBS_ROOT", t.dir.path())
        .args(["apps", "list"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    drop(child.stdout.take());
    let out = child.wait_with_output().unwrap();
    assert_eq!(out.status.code(), Some(0));
    assert!(
        out.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn apps_show_partial_failure_exit_3() {
    let t = fixture(FLATPAK_FAIL);
    crumbs(&t)
        .args(["apps", "show", "firefox"])
        .assert()
        .code(3)
        .stdout(predicate::str::contains("pacman:firefox"));
}

#[test]
fn apps_show_installation_uses_serde_name() {
    let t = fixture(FLATPAK_OK);
    crumbs(&t)
        .args(["apps", "show", "firefox"])
        .assert()
        .stdout(predicate::str::is_match(r"Installation:\s+system\n").unwrap());
}

#[test]
fn apps_show_lists_leftovers() {
    let t = fixture(FLATPAK_OK);
    with_leftovers(&t);
    let text = snapshot_text(&t, &["apps", "show", "Tool"]);
    let text = without_dir_blocks(
        &t,
        text,
        &[
            ("~/.cache/Tool", 12 * 1024, 1),
            ("~/.config/Tool", 8 * 1024, 1),
            ("2 leftovers", 20 * 1024, 2),
        ],
    );
    insta::assert_snapshot!(text);
}

#[test]
fn apps_show_json_leftovers() {
    let t = fixture(FLATPAK_OK);
    with_leftovers(&t);
    let v = stdout_json(crumbs(&t).args(["apps", "show", "Tool", "--json"]));
    let l = v["data"]["leftovers"].as_array().unwrap();
    assert_eq!(l.len(), 2, "{l:?}");
    assert_eq!(l[0]["confidence"], "high");
    assert_eq!(l[0]["path"], "/home/user/.cache/Tool");
    assert!(l[0].get("fingerprint").is_none());
    assert_eq!(l[0]["shared"], false);
}

#[test]
fn apps_list_sanitises_names_and_ids() {
    let (t, _) = evil_fixture();
    let out = crumbs(&t).args(["apps", "list"]).output().unwrap();
    assert_eq!(out.status.code(), Some(0));
    assert_clean(&out);
    assert_shows_replacement(&out);
}

#[test]
fn apps_list_json_keeps_raw_name() {
    let (t, id) = evil_fixture();
    assert!(id.contains('\r') || id.contains('\x1b'));
    let j = stdout_json(crumbs(&t).args(["apps", "list", "--json"]));
    let names: Vec<&str> = j["data"]["apps"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["display_name"].as_str().unwrap())
        .collect();
    assert!(names.iter().any(|n| n.contains('\x1b')), "{names:?}");
}

#[test]
fn apps_show_sanitises_name_and_id() {
    let (t, id) = evil_fixture();
    let out = crumbs(&t).args(["apps", "show", &id]).output().unwrap();
    assert_eq!(out.status.code(), Some(0));
    assert_clean(&out);
    assert_shows_replacement(&out);
}

#[test]
fn apps_show_lists_system_leftovers() {
    let t = inky_pkg_fixture();
    // Resembles the app's name only: low.
    t.write("/etc/Inky-2/x", b"x");
    let (code, stdout, stderr) = run_out(crumbs(&t).args(["apps", "show", "inky"]));
    assert_eq!(code, 0, "{stdout}{stderr}");
    let row = |kind: &str, path: &str| {
        stdout.lines().any(|l| {
            let w: Vec<&str> = l.split_whitespace().collect();
            w.ends_with(&[kind, path, "root"])
        })
    };
    assert!(row("system-config", "/etc/inky"), "{stdout}");
    assert!(row("system-data", "/opt/inky"), "{stdout}");
    assert!(!stdout.contains("/etc/Inky-2"), "{stdout}");
    let (_, stdout, _) = run_out(crumbs(&t).args(["apps", "show", "inky", "--show-low"]));
    assert!(stdout.contains("/etc/Inky-2"), "{stdout}");
    let v = stdout_json(crumbs(&t).args(["apps", "show", "inky", "--json"]));
    let l = v["data"]["leftovers"].as_array().unwrap();
    let sys: Vec<&serde_json::Value> = l.iter().filter(|f| f["scope"] == "system-tree").collect();
    assert_eq!(sys.len(), 2, "{v}");
    assert!(sys.iter().all(|f| f["preselected"] == false), "{v}");
    assert!(sys.iter().any(|f| f["path"] == "/etc/inky"), "{v}");
    let v = stdout_json(crumbs(&t).args(["apps", "show", "inky", "--json", "--show-low"]));
    assert!(v.to_string().contains("/etc/Inky-2"), "{v}");
}

#[test]
fn apps_list_json_has_icon_from_desktop_entry() {
    let t = fixture(FLATPAK_OK);
    t.desktop(
        "/usr/share/applications/firefox.desktop",
        "[Desktop Entry]\nType=Application\nName=Firefox\nIcon=firefox\nExec=/usr/bin/firefox %u\n",
    );
    let v = stdout_json(crumbs(&t).args(["apps", "list", "--json"]));
    let apps = v["data"]["apps"].as_array().unwrap();
    let firefox = apps.iter().find(|a| a["name"] == "firefox").unwrap();
    assert_eq!(firefox["icon"], "firefox", "{firefox}");
    let xemu = apps
        .iter()
        .find(|a| a["id"].as_str().unwrap().contains("app.xemu.xemu"))
        .unwrap();
    assert_eq!(
        xemu["icon"], "app.xemu.xemu",
        "a Flatpak falls back to its id"
    );
}

#[test]
fn doctor_json() {
    let t = fixture(FLATPAK_OK);
    let v = stdout_json(crumbs(&t).args(["doctor", "--json"]));
    assert_eq!(v["command"], "doctor");
    assert!(v["data"]["escalation"].is_null());
    assert_eq!(v["data"]["sources"][0]["source"], "pacman");
}

#[test]
fn doctor_human() {
    let t = fixture(FLATPAK_OK);
    crumbs(&t)
        .arg("doctor")
        .assert()
        .code(0)
        .stdout(predicate::str::contains("✓ pacman"))
        .stdout(predicate::str::contains("Escalation: none\n"))
        .stdout(predicate::str::contains("privileged cleanup: unavailable"));
}

#[test]
fn doctor_counts_configured_appimage_dirs() {
    let t = fixture(FLATPAK_OK);
    let count = |t: &FixtureTree| -> u64 {
        let v = stdout_json(crumbs(t).args(["doctor", "--json"]));
        let s = v["data"]["sources"].as_array().unwrap();
        s.iter().find(|s| s["source"] == "appimage").unwrap()["app_count"]
            .as_u64()
            .unwrap()
    };
    assert_eq!(count(&t), 1);
    t.write(
        "/home/user/.config/crumbs/config.toml",
        b"appimage_dirs = [\"~/Tools\"]\n",
    );
    t.appimage("/home/user/Tools/Extra-2.0-x86_64.AppImage");
    assert_eq!(count(&t), 2);
}
