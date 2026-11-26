use super::*;

#[test]
fn uninstall_unmanaged_vendor_dir_exit_2_and_leftovers_only_works() {
    let t = inky_unmanaged_fixture();
    // A program in a vendor directory not named after the app.
    t.write_exec("/opt/vendor/bin/tool", b"");
    t.desktop(
        "/usr/share/applications/zedbar.desktop",
        "[Desktop Entry]\nType=Application\nName=Zed Bar\nExec=/opt/vendor/bin/tool\n",
    );
    t.write("/home/user/.cache/zedbar/blob", b"x");
    let (code, _, stderr) = run_out(sh_runner(&t).args(["uninstall", "Zed Bar", "--yes"]));
    assert_eq!(code, 2, "{stderr}");
    assert!(stderr.contains("does not match the app's name"), "{stderr}");
    assert!(stderr.contains("use --leftovers-only"), "{stderr}");
    assert!(real(&t, "/opt/vendor/bin/tool").exists());
    assert!(real(&t, "/home/user/.cache/zedbar/blob").exists());
    let v = json_error(&t, &["uninstall", "Zed Bar", "--json", "--yes"], 2);
    let msg = v["errors"].as_array().unwrap().last().unwrap()["message"].to_string();
    assert!(msg.contains("does not match the app's name"), "{msg}");
    let (code, stdout, stderr) =
        run_out(crumbs(&t).args(["uninstall", "Zed Bar", "--leftovers-only", "--yes"]));
    assert_eq!(code, 0, "{stdout}{stderr}");
    assert!(!real(&t, "/home/user/.cache/zedbar").exists());
    assert!(real(&t, "/opt/vendor/bin/tool").exists());
}

/// The CLI half of `build_uninstall_matches_cli_plan` (cmd/build.rs): the
/// same fixture and the same expected actions, so the builder the `mcp`
/// server calls and `uninstall --dry-run --json` plan alike.
#[test]
fn uninstall_dry_run_json_plan_matches_builder() {
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
    let out = crumbs(&t)
        .args([
            "uninstall",
            "inky",
            "--leftovers-only",
            "--dry-run",
            "--json",
        ])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["data"]["report"], serde_json::Value::Null);
    let plans = v["data"]["plans"].as_array().unwrap();
    assert_eq!(plans.len(), 1);
    let mut actions: Vec<(String, String)> = plans[0]["actions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| {
            (
                a["action"]["type"].as_str().unwrap().to_string(),
                a["action"]["path"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    actions.sort();
    let want = [
        ("trash", "/home/user/.cache/inky"),
        ("trash", "/home/user/.config/inky"),
    ]
    .map(|(k, p)| (k.to_string(), p.to_string()));
    assert_eq!(actions, want, "{:#}", plans[0]);
}

// ---- packages that are part of the uQ desktop ----

#[test]
fn uninstall_uq_package_needs_desktop_flag() {
    let t = pacman_fixture(PACMAN_OK);
    t.pacman_pkg("scdr", "0.2.0-8", Some(1_000), &["usr/bin/scdrd"]);
    // Without a terminal, --yes alone is not enough: refused, nothing run.
    let (code, stdout, stderr) = run_out(crumbs(&t).env("CRUMBS_TEST_PRIV_RUNNER", "sh").args([
        "uninstall",
        "scdr",
        "--yes",
    ]));
    assert_eq!(code, 1, "{stdout}{stderr}");
    assert!(
        stderr.contains("pacman:scdr is part of the uQ desktop; re-run with --desktop"),
        "{stderr}"
    );
    assert!(pkg_present(&t, "scdr-0.2.0-8"));
    assert!(!real(&t, "/pacman-args").exists());
    // A dry run is refused the same way without a terminal (the MCP server).
    let (code, _, _) = run_out(crumbs(&t).args(["uninstall", "scdr", "--dry-run"]));
    assert_eq!(code, 1);
    // --leftovers-only keeps the package, so it needs no --desktop.
    let (code, stdout, stderr) =
        run_out(crumbs(&t).args(["uninstall", "scdr", "--leftovers-only", "--dry-run"]));
    assert_eq!(code, 0, "{stdout}{stderr}");
    // With --desktop the plan removes it.
    let v = stdout_json(crumbs(&t).args(["uninstall", "scdr", "--desktop", "--dry-run", "--json"]));
    let actions = v["data"]["plans"][0]["actions"].as_array().unwrap();
    assert_eq!(actions[0]["action"]["type"], "package-remove", "{v}");
    // apps show and the JSON both say which desktop it belongs to.
    let v = stdout_json(crumbs(&t).args(["apps", "show", "scdr", "--json"]));
    assert_eq!(v["data"]["app"]["part_of"], "uQ", "{v}");
    let v = stdout_json(crumbs(&t).args(["apps", "show", "firefox", "--json"]));
    assert_eq!(v["data"]["app"]["part_of"], serde_json::Value::Null, "{v}");
}

// ---- uninstall --desktop-id (uQ's "Uninstall…" app action) ----

#[test]
fn uninstall_by_desktop_id_resolves_the_launchers_app() {
    let t = fixture(FLATPAK_OK);
    t.write("/home/user/.cache/firefox/blob", b"x");
    let v = stdout_json(crumbs(&t).args([
        "uninstall",
        "--desktop-id",
        "firefox.desktop",
        "--leftovers-only",
        "--dry-run",
        "--json",
    ]));
    assert_eq!(v["command"], "uninstall");
    assert_eq!(v["data"]["plans"][0]["app_id"], "pacman:firefox", "{v}");
    // Without a terminal and without --yes it asks for nothing and stops,
    // like any other uninstall.
    let (code, stdout, stderr) =
        run_out(crumbs(&t).args(["uninstall", "--desktop-id", "firefox.desktop"]));
    assert_eq!(code, 2, "{stdout}{stderr}");
    assert!(real(&t, "/home/user/.cache/firefox").exists());
    assert!(real(&t, "/usr/bin/firefox").exists());
}

#[test]
fn uninstall_by_desktop_id_refuses_bad_and_unknown_ids() {
    let t = fixture(FLATPAK_OK);
    for (id, said) in [
        ("--desktop-id=-rf.desktop", "is not a desktop file id"),
        (
            "--desktop-id=../firefox.desktop",
            "is not a desktop file id",
        ),
        ("--desktop-id=firefox", "is not a desktop file id"),
        (
            "--desktop-id=Firefox Web Browser",
            "is not a desktop file id",
        ),
        // An app id or name is not a launcher id.
        ("--desktop-id=pacman:firefox", "is not a desktop file id"),
        (
            "--desktop-id=nope.desktop",
            "no installed app owns the launcher",
        ),
    ] {
        let (code, stdout, stderr) = run_out(crumbs(&t).args(["uninstall", id, "--dry-run"]));
        assert_eq!(code, 2, "{id}: {stdout}{stderr}");
        assert!(stderr.contains(said), "{id}: {stderr}");
    }
    let v = json_error(
        &t,
        &[
            "uninstall",
            "--desktop-id=-x.desktop",
            "--json",
            "--dry-run",
        ],
        2,
    );
    assert_eq!(v["command"], "uninstall");
    // Apps and --desktop-id don't mix; one of them is needed.
    crumbs(&t)
        .args(["uninstall", "firefox", "--desktop-id", "firefox.desktop"])
        .assert()
        .code(2);
    crumbs(&t).args(["uninstall"]).assert().code(2);
}

#[test]
fn uninstall_by_desktop_id_keeps_uq_protection() {
    let t = pacman_fixture(PACMAN_OK);
    t.pacman_pkg(
        "scdr",
        "0.2.0-8",
        Some(1_000),
        &[
            "usr/bin/scdrd",
            "usr/share/applications/",
            "usr/share/applications/scdr.desktop",
        ],
    );
    t.desktop(
        "/usr/share/applications/scdr.desktop",
        "[Desktop Entry]\nType=Application\nName=scdr\nExec=/usr/bin/scdrd\n",
    );
    let (code, stdout, stderr) = run_out(crumbs(&t).env("CRUMBS_TEST_PRIV_RUNNER", "sh").args([
        "uninstall",
        "--desktop-id",
        "scdr.desktop",
        "--yes",
    ]));
    assert_eq!(code, 1, "{stdout}{stderr}");
    assert!(
        stderr.contains("pacman:scdr is part of the uQ desktop; re-run with --desktop"),
        "{stderr}"
    );
    assert!(pkg_present(&t, "scdr-0.2.0-8"));
    assert!(!real(&t, "/pacman-args").exists());
}

#[test]
fn history_lists_batch() {
    let t = fixture(FLATPAK_OK);
    with_leftovers(&t);
    crumbs(&t)
        .args(["uninstall", "Tool", "--yes"])
        .assert()
        .code(0);
    let h = history_json(&t);
    let batch = h["data"]["batches"][0]["batch"]
        .as_str()
        .unwrap()
        .to_string();
    let out = crumbs(&t).arg("history").output().unwrap();
    assert_eq!(out.status.code(), Some(0));
    let text = String::from_utf8(out.stdout).unwrap();
    let line = text.lines().find(|l| l.starts_with(&batch)).unwrap();
    assert!(
        line.contains("appimage:~/Applications/Tool-1.0-x86_64.AppImage"),
        "{line}"
    );
    assert!(line.contains("3 items"), "{line}");
    assert!(!line.contains("restored"), "{line}");
    crumbs(&t).args(["restore", &batch]).assert().code(0);
    let text = String::from_utf8(crumbs(&t).arg("history").output().unwrap().stdout).unwrap();
    let line = text.lines().find(|l| l.starts_with(&batch)).unwrap();
    assert!(line.ends_with("[restored 3]"), "{text}");
}

#[test]
fn history_labels_batches_by_kind() {
    let t = scan_fixture(FLATPAK_OK);
    with_orphans(&t);
    let (code, stdout, _) = run_clean(&t, &["cache", "--yes"]);
    assert_eq!(code, 0, "{stdout}");
    crumbs(&t)
        .args(["orphans", "--clean", "--select", "~/.thunderbird", "--yes"])
        .assert()
        .code(0);
    gutcheck(&t);
    crumbs(&t)
        .args(["uninstall", "GutCheck", "--yes"])
        .assert()
        .code(0);
    let h = history_json(&t);
    let batches = h["data"]["batches"].as_array().unwrap();
    let kinds: Vec<_> = batches
        .iter()
        .map(|b| b["entries"][0]["kind"].as_str().unwrap_or("-"))
        .collect();
    assert_eq!(kinds, ["uninstall", "orphans", "clean"], "{h}");
    let labels: Vec<_> = batches
        .iter()
        .map(|b| b["label"].as_str().unwrap())
        .collect();
    assert_eq!(
        labels,
        [
            "appimage:~/Applications/GutCheck-0.19.0-x86_64.AppImage",
            "orphans",
            "clean"
        ],
        "{h}"
    );
    // A line written before `kind` existed keeps the old label.
    let log = real(&t, "/home/user/.local/state/crumbs/history.jsonl");
    let mut text = std::fs::read_to_string(&log).unwrap();
    text.push_str(r#"{"id":"1-0","batch":"1","time":"2026-01-01T00:00:00Z","action":"trash","path":"/home/user/x","trashed":null,"backup":null,"size":1,"ok":true,"error":null,"app_id":null}"#);
    text.push('\n');
    std::fs::write(&log, text).unwrap();
    let out = crumbs(&t).arg("history").output().unwrap();
    let text = String::from_utf8(out.stdout).unwrap();
    let line = text.lines().find(|l| l.starts_with("1  ")).unwrap();
    assert!(line.contains("  orphans  "), "{text}");
    let clean = text
        .lines()
        .find(|l| l.contains("  clean  "))
        .unwrap_or_else(|| panic!("{text}"));
    assert!(clean.contains("1 items"), "{clean}");
}

#[test]
fn history_sanitises_label() {
    let (t, id) = evil_fixture();
    crumbs(&t)
        .args(["uninstall", &id, "--yes"])
        .assert()
        .code(0);
    let out = crumbs(&t).arg("history").output().unwrap();
    assert_eq!(out.status.code(), Some(0));
    assert_clean(&out);
    assert_shows_replacement(&out);
}
