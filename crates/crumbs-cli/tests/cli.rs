#![allow(clippy::unwrap_used, clippy::expect_used)]

use assert_cmd::Command;
use crumbs_core::testing::FixtureTree;
use predicates::prelude::*;

const FLATPAK_OK: &str = "#!/bin/sh\nprintf 'app.xemu.xemu\\txemu\\t0.8.136\\tuser\\t34.4 MB\\ncom.dosbox_x.DOSBox-X\\tDOSBox-X\\t2026.10.01\\tsystem\\t51.4 MB\\ncom.obsproject.Studio\\tOBS Studio\\t33.0.0\\tsystem\\t593.5 MB\\n'\n";
const FLATPAK_FAIL: &str = "#!/bin/sh\necho boom >&2\nexit 1\n";

fn fixture(flatpak_script: &str) -> FixtureTree {
    let t = FixtureTree::new();
    t.os_release("manjaro", "arch", "Manjaro Linux");
    t.pacman_pkg(
        "firefox",
        "159.0.1-1",
        Some(300_000_000),
        &[
            "usr/bin/firefox",
            "usr/share/applications/",
            "usr/share/applications/firefox.desktop",
        ],
    );
    t.pacman_pkg("coreutils", "9.5-1", Some(2_000_000), &["usr/bin/ls"]);
    t.desktop(
        "/usr/share/applications/firefox.desktop",
        "[Desktop Entry]\nType=Application\nName=Firefox\nExec=/usr/bin/firefox %u\n",
    );
    t.write_exec("/usr/bin/firefox", b"");
    t.write_exec("/usr/bin/ls", b"");
    t.appimage("/home/user/Applications/Tool-1.0-x86_64.AppImage");
    t.write_exec("/opt/foo/foo", b"");
    t.desktop(
        "/usr/share/applications/foo.desktop",
        "[Desktop Entry]\nType=Application\nName=Zed Foo\nExec=/opt/foo/foo\n",
    );
    t.write_exec("/usr/bin/flatpak", flatpak_script.as_bytes());
    t
}

fn crumbs(t: &FixtureTree) -> Command {
    let mut cmd = assert_cmd::cargo::cargo_bin_cmd!("crumbs");
    cmd.env("CRUMBS_ROOT", t.dir.path());
    cmd.env_remove("NO_COLOR");
    cmd
}

fn stdout_json(cmd: &mut Command) -> serde_json::Value {
    let out = cmd.output().unwrap();
    serde_json::from_slice(&out.stdout).unwrap()
}

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

fn regex_escape(s: &str) -> String {
    s.chars()
        .flat_map(|c| {
            if c.is_ascii_alphanumeric() || c == '/' || c == '_' {
                vec![c]
            } else {
                vec!['\\', c]
            }
        })
        .collect()
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
        .stdout(predicate::str::contains(
            "none (privileged cleanup unavailable)",
        ));
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

fn with_leftovers(t: &FixtureTree) {
    t.write("/home/user/.config/Tool/settings.json", &[b'x'; 5000]);
    t.write("/home/user/.cache/Tool/blob", &[b'y'; 9000]);
}

fn with_orphans(t: &FixtureTree) {
    t.write("/home/user/.thunderbird/profiles.ini", b"x");
    t.mkdir("/home/user/.var/app/org.gone.App");
    t.mkdir("/home/user/.config/zzquux-thing");
    // Orphans modified in the last 30 days are capped at low: make them old.
    let old = std::time::SystemTime::now() - std::time::Duration::from_secs(60 * 86_400);
    for d in [
        "/home/user/.thunderbird",
        "/home/user/.var/app/org.gone.App",
        "/home/user/.config/zzquux-thing",
    ] {
        std::fs::File::open(t.mkdir(d))
            .unwrap()
            .set_modified(old)
            .unwrap();
    }
}

fn snapshot_text(t: &FixtureTree, args: &[&str]) -> String {
    let out = crumbs(t).args(args).output().unwrap();
    assert_eq!(out.status.code(), Some(0));
    let text = String::from_utf8(out.stdout).unwrap();
    text.replace(&t.dir.path().to_string_lossy().into_owned(), "[ROOT]")
}

#[test]
fn apps_show_lists_leftovers() {
    let t = fixture(FLATPAK_OK);
    with_leftovers(&t);
    insta::assert_snapshot!(snapshot_text(&t, &["apps", "show", "Tool"]));
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
fn orphans_lists_medium() {
    let t = fixture(FLATPAK_OK);
    with_orphans(&t);
    insta::assert_snapshot!(snapshot_text(&t, &["orphans"]));
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

// ---- uninstall, orphans --clean, history, restore (all inside the fixture) ----

fn real(t: &FixtureTree, in_system: &str) -> std::path::PathBuf {
    t.dir.path().join(in_system.trim_start_matches('/'))
}

fn trash_files(t: &FixtureTree) -> Vec<String> {
    let dir = real(t, "/home/user/.local/share/Trash/files");
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .map(|d| {
            d.flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

fn gutcheck(t: &FixtureTree) {
    t.appimage("/home/user/Applications/GutCheck-0.19.0-x86_64.AppImage");
    t.write("/home/user/.config/GutCheck/settings.json", &[b'x'; 5000]);
    t.write("/home/user/.cache/GutCheck/blob", &[b'y'; 9000]);
}

fn history_json(t: &FixtureTree) -> serde_json::Value {
    stdout_json(crumbs(t).args(["history", "--json"]))
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
fn uninstall_pacman_requires_leftovers_only_exit_2() {
    let t = fixture(FLATPAK_OK);
    t.write("/home/user/.cache/firefox/blob", b"x");
    crumbs(&t)
        .args(["uninstall", "firefox", "--yes"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains(
            "removing pacman apps needs privileged support (coming in a later version); use --leftovers-only",
        ));
    assert!(real(&t, "/home/user/.cache/firefox/blob").exists());
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

#[test]
fn uninstall_normalized_match_exit_2_with_suggestion() {
    let t = fixture(FLATPAK_OK);
    with_leftovers(&t);
    crumbs(&t)
        .args(["uninstall", "T-o-o-l", "--yes"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains(
            "did you mean 'appimage:~/Applications/Tool-1.0-x86_64.AppImage'?",
        ));
    assert!(real(&t, "/home/user/.config/Tool").exists());
    assert!(real(&t, "/home/user/Applications/Tool-1.0-x86_64.AppImage").exists());
}

#[test]
fn uninstall_refuses_on_source_errors_without_force() {
    let t = fixture(FLATPAK_FAIL);
    with_leftovers(&t);
    crumbs(&t)
        .args(["uninstall", "Tool", "--yes"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains(
            "refusing: some sources failed (flatpak); re-run with --force to continue",
        ));
    assert!(real(&t, "/home/user/.config/Tool").exists());
    crumbs(&t)
        .args(["uninstall", "Tool", "--yes", "--force"])
        .assert()
        .code(0);
    assert!(!real(&t, "/home/user/.config/Tool").exists());
}

#[test]
fn uninstall_non_interactive_without_yes_exit_2() {
    let t = fixture(FLATPAK_OK);
    with_leftovers(&t);
    crumbs(&t)
        .args(["uninstall", "Tool"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains(
            "refusing to act without --yes in a non-interactive session",
        ));
    assert!(real(&t, "/home/user/.config/Tool").exists());
}

#[test]
fn uninstall_keep_data_only_cache() {
    let t = fixture(FLATPAK_OK);
    with_leftovers(&t);
    crumbs(&t)
        .args([
            "uninstall",
            "Tool",
            "--leftovers-only",
            "--keep-data",
            "--yes",
        ])
        .assert()
        .code(0);
    assert!(!real(&t, "/home/user/.cache/Tool").exists());
    assert!(real(&t, "/home/user/.config/Tool/settings.json").exists());
    assert!(real(&t, "/home/user/Applications/Tool-1.0-x86_64.AppImage").exists());
    assert_eq!(trash_files(&t), ["Tool"]);
}

#[test]
fn uninstall_json_requires_yes_or_dry_run() {
    let t = fixture(FLATPAK_OK);
    with_leftovers(&t);
    crumbs(&t)
        .args(["uninstall", "Tool", "--json"])
        .assert()
        .code(2);
    assert!(real(&t, "/home/user/.config/Tool").exists());
    let v = stdout_json(crumbs(&t).args(["uninstall", "Tool", "--json", "--dry-run"]));
    assert_eq!(v["command"], "uninstall");
    assert_eq!(
        v["data"]["plans"][0]["actions"].as_array().unwrap().len(),
        3
    );
    assert!(v["data"]["report"].is_null());
    assert!(real(&t, "/home/user/.config/Tool").exists());
    let v = stdout_json(crumbs(&t).args(["uninstall", "Tool", "--json", "--yes"]));
    let results = v["data"]["report"][0]["results"].as_array().unwrap();
    assert_eq!(results.len(), 3);
    assert!(results.iter().all(|r| r["ok"] == true), "{v}");
    assert!(!real(&t, "/home/user/.config/Tool").exists());
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

fn json_error(t: &FixtureTree, args: &[&str], code: i32) -> serde_json::Value {
    let out = crumbs(t).args(args).output().unwrap();
    assert_eq!(out.status.code(), Some(code));
    assert!(!out.stderr.is_empty(), "human text stays on stderr");
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["schema_version"], 1);
    let errors = v["errors"].as_array().unwrap();
    assert!(errors.iter().any(|e| e["source"] == "cli"), "{v}");
    v
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

fn gutcheck_launcher(t: &FixtureTree) {
    gutcheck(t);
    t.desktop(
        "/home/user/.local/share/applications/appofholding-gutcheck.desktop",
        "[Desktop Entry]\nType=Application\nName=GutCheck\nExec=/home/user/Applications/GutCheck-0.19.0-x86_64.AppImage\nIcon=appofholding-gutcheck\n",
    );
    t.write(
        "/home/user/.local/share/icons/hicolor/512x512/apps/appofholding-gutcheck.png",
        b"png",
    );
}

#[test]
fn uninstall_leftovers_only_keeps_own_launcher() {
    let t = fixture(FLATPAK_OK);
    gutcheck_launcher(&t);
    let full = String::from_utf8(
        crumbs(&t)
            .args(["uninstall", "GutCheck", "--dry-run"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    assert!(full.contains("appofholding-gutcheck.desktop"), "{full}");
    assert!(full.contains("appofholding-gutcheck.png"), "{full}");
    let out = crumbs(&t)
        .args(["uninstall", "GutCheck", "--leftovers-only", "--yes"])
        .output()
        .unwrap();
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert_eq!(out.status.code(), Some(0), "{stdout}");
    assert!(!stdout.contains("appofholding-gutcheck"), "{stdout}");
    assert!(!real(&t, "/home/user/.config/GutCheck").exists());
    assert!(
        real(
            &t,
            "/home/user/.local/share/applications/appofholding-gutcheck.desktop"
        )
        .exists()
    );
    assert!(
        real(
            &t,
            "/home/user/.local/share/icons/hicolor/512x512/apps/appofholding-gutcheck.png"
        )
        .exists()
    );
    assert!(
        real(
            &t,
            "/home/user/Applications/GutCheck-0.19.0-x86_64.AppImage"
        )
        .exists()
    );
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
fn uninstall_never_trashes_parent_of_excluded_path() {
    let t = fixture(FLATPAK_OK);
    t.appimage("/home/user/Applications/Foobar-1.0-x86_64.AppImage");
    t.write("/home/user/.config/Foobar/keep/important", b"precious");
    t.write("/home/user/.config/Foobar/other", b"x");
    t.write(
        "/home/user/.config/crumbs/config.toml",
        b"exclude = [\"~/.config/Foobar/keep\"]\n",
    );
    let out = crumbs(&t)
        .args(["uninstall", "Foobar", "--yes"])
        .output()
        .unwrap();
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(!stdout.contains("~/.config/Foobar"), "{stdout}");
    assert_eq!(
        std::fs::read(real(&t, "/home/user/.config/Foobar/keep/important")).unwrap(),
        b"precious"
    );
    assert!(real(&t, "/home/user/.config/Foobar/other").exists());
}

/// An AppImage whose file can't pass `safety::verify` is refused as a
/// whole before anything is previewed or touched; its leftovers stay too.
#[test]
fn uninstall_refuses_appimage_in_symlinked_dir() {
    let t = fixture(FLATPAK_OK);
    std::fs::rename(
        real(&t, "/home/user/Applications"),
        real(&t, "/home/user/apps-real"),
    )
    .unwrap();
    t.appimage("/home/user/apps-real/GutCheck-0.19.0-x86_64.AppImage");
    t.symlink("/home/user/Applications", "apps-real");
    t.write("/home/user/.config/GutCheck/settings.json", b"x");
    let out = crumbs(&t)
        .args(["uninstall", "GutCheck", "--yes"])
        .output()
        .unwrap();
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert_eq!(out.status.code(), Some(1), "{stderr}");
    assert!(
        stderr.contains(
            "error: cannot remove appimage:~/Applications/GutCheck-0.19.0-x86_64.AppImage: \
             refusing: ~/Applications is a symlink; use --leftovers-only to remove only its leftovers"
        ),
        "{stderr}"
    );
    assert!(
        out.stdout.is_empty(),
        "{:?}",
        String::from_utf8_lossy(&out.stdout)
    );
    assert!(real(&t, "/home/user/.config/GutCheck/settings.json").exists());
    assert!(real(&t, "/home/user/apps-real/GutCheck-0.19.0-x86_64.AppImage").exists());
    assert!(trash_files(&t).is_empty());

    let v = json_error(&t, &["uninstall", "GutCheck", "--json", "--yes"], 1);
    assert!(v.to_string().contains("~/Applications is a symlink"), "{v}");
    assert!(real(&t, "/home/user/.config/GutCheck/settings.json").exists());

    // --leftovers-only never looks at the file.
    crumbs(&t)
        .args(["uninstall", "GutCheck", "--leftovers-only", "--yes"])
        .assert()
        .code(0);
    assert!(!real(&t, "/home/user/.config/GutCheck").exists());
    assert!(real(&t, "/home/user/apps-real/GutCheck-0.19.0-x86_64.AppImage").exists());
}

#[test]
fn uninstall_refuses_appimage_outside_home() {
    let t = fixture(FLATPAK_OK);
    t.appimage("/opt/apps/Foo-1.0-x86_64.AppImage");
    t.write("/home/user/.config/Foo/settings.json", b"x");
    t.write(
        "/home/user/.config/crumbs/config.toml",
        b"appimage_dirs = [\"/opt/apps\"]\n",
    );
    let out = crumbs(&t)
        .args([
            "uninstall",
            "appimage:/opt/apps/Foo-1.0-x86_64.AppImage",
            "--yes",
        ])
        .output()
        .unwrap();
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert_eq!(out.status.code(), Some(1), "{stderr}");
    assert!(
        stderr.contains(
            "error: cannot remove appimage:/opt/apps/Foo-1.0-x86_64.AppImage: \
             refusing: /opt/apps/Foo-1.0-x86_64.AppImage is outside the allowed roots; \
             use --leftovers-only to remove only its leftovers"
        ),
        "{stderr}"
    );
    assert!(real(&t, "/home/user/.config/Foo/settings.json").exists());
    assert!(real(&t, "/opt/apps/Foo-1.0-x86_64.AppImage").exists());
}

#[test]
fn preview_lists_lines_an_edit_removes() {
    let t = fixture(FLATPAK_OK);
    t.appimage("/home/user/Applications/GutCheck-0.19.0-x86_64.AppImage");
    t.desktop(
        "/home/user/.local/share/applications/gutcheck.desktop",
        "[Desktop Entry]\nType=Application\nName=GutCheck\nExec=/home/user/Applications/GutCheck-0.19.0-x86_64.AppImage\n",
    );
    t.write(
        "/home/user/.config/mimeapps.list",
        b"[Default Applications]\ntext/x-gut=gutcheck.desktop\nimage/png=other.desktop\n\n[Added Associations]\nx-scheme-handler/gut=gutcheck.desktop;\n",
    );
    let out = crumbs(&t)
        .args(["uninstall", "GutCheck", "--dry-run"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(
        text.contains(
            "file-type associations\n    - text/x-gut=gutcheck.desktop\n    - x-scheme-handler/gut=gutcheck.desktop;\n"
        ),
        "{text}"
    );
    insta::assert_snapshot!(text);
}

/// With `--leftovers-only` the app stays, so do its file-type associations.
#[test]
fn uninstall_leftovers_only_keeps_mime_associations() {
    let t = fixture(FLATPAK_OK);
    let mime = b"[Default Applications]\ntext/html=firefox.desktop\n";
    t.write("/home/user/.config/mimeapps.list", mime);
    t.write("/home/user/.cache/firefox/blob", b"x");
    let out = crumbs(&t)
        .args(["uninstall", "firefox", "--leftovers-only", "--yes"])
        .output()
        .unwrap();
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert_eq!(out.status.code(), Some(0), "{stdout}");
    assert!(!stdout.contains("mimeapps.list"), "{stdout}");
    assert!(!real(&t, "/home/user/.cache/firefox").exists());
    assert_eq!(
        std::fs::read(real(&t, "/home/user/.config/mimeapps.list")).unwrap(),
        mime
    );
    let names: Vec<String> = std::fs::read_dir(real(&t, "/home/user/.config"))
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert!(!names.iter().any(|n| n.contains("crumbs-bak")), "{names:?}");
}

// ---- M3 task 4: CLI consistency ----

fn bad_config(t: &FixtureTree) {
    t.write("/home/user/.config/crumbs/config.toml", b"protect = [\n");
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

#[test]
fn source_filter_scopes_errors() {
    let t = fixture(FLATPAK_FAIL);
    let out = crumbs(&t)
        .args(["apps", "list", "--source", "pacman"])
        .output()
        .unwrap();
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert_eq!(out.status.code(), Some(0), "{stderr}");
    assert!(!stderr.contains("flatpak"), "{stderr}");
    let v = stdout_json(crumbs(&t).args(["apps", "list", "--source", "pacman", "--json"]));
    assert!(v["errors"].as_array().unwrap().is_empty(), "{v}");
    // A failed source that was asked for still reports.
    crumbs(&t)
        .args(["apps", "list", "--source", "flatpak"])
        .assert()
        .code(3)
        .stderr(predicate::str::contains("warning: flatpak:"));
}

fn batch_ids(t: &FixtureTree) -> Vec<String> {
    history_json(t)["data"]["batches"]
        .as_array()
        .unwrap()
        .iter()
        .map(|b| b["batch"].as_str().unwrap().to_string())
        .collect()
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

fn json_error_free(t: &FixtureTree, args: &[&str], code: i32) -> serde_json::Value {
    let out = crumbs(t).args(args).output().unwrap();
    assert_eq!(out.status.code(), Some(code));
    serde_json::from_slice(&out.stdout).unwrap()
}

#[test]
fn dotfile_kind() {
    let t = fixture(FLATPAK_OK);
    t.write("/home/user/.zzquuxrc", b"x");
    t.write("/home/user/.zzquuxdir/a", b"x");
    let v = stdout_json(crumbs(&t).args(["orphans", "--show-low", "--json"]));
    let kind_of = |name: &str| -> String {
        let want = format!("/home/user/{name}");
        v["data"]["orphans"]
            .as_array()
            .unwrap()
            .iter()
            .find(|f| f["path"] == want.as_str())
            .unwrap_or_else(|| panic!("{want} not in {v}"))["kind"]
            .as_str()
            .unwrap()
            .to_string()
    };
    assert_eq!(kind_of(".zzquuxrc"), "dot-file");
    assert_eq!(kind_of(".zzquuxdir"), "dot-dir");
    crumbs(&t)
        .args(["orphans", "--show-low"])
        .assert()
        .stdout(predicate::str::contains("dot-file"));
}

#[test]
fn keep_data_removes_own_launchers_with_appimage() {
    let t = fixture(FLATPAK_OK);
    gutcheck_launcher(&t);
    // Someone else's launcher that mentions the app stays.
    t.desktop(
        "/home/user/.local/share/applications/gutcheck-helper.desktop",
        "[Desktop Entry]\nType=Application\nName=GutCheck Helper\nExec=/usr/bin/gutcheck-helper\n",
    );
    let out = crumbs(&t)
        .args(["uninstall", "GutCheck", "--keep-data", "--yes"])
        .output()
        .unwrap();
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert_eq!(out.status.code(), Some(0), "{stdout}");
    assert!(!real(&t, "/home/user/.cache/GutCheck").exists());
    assert!(
        !real(
            &t,
            "/home/user/Applications/GutCheck-0.19.0-x86_64.AppImage"
        )
        .exists()
    );
    // Nothing is left pointing at the removed file.
    assert!(
        !real(
            &t,
            "/home/user/.local/share/applications/appofholding-gutcheck.desktop"
        )
        .exists(),
        "{stdout}"
    );
    assert!(
        !real(
            &t,
            "/home/user/.local/share/icons/hicolor/512x512/apps/appofholding-gutcheck.png"
        )
        .exists(),
        "{stdout}"
    );
    // Data and settings stay.
    assert!(real(&t, "/home/user/.config/GutCheck/settings.json").exists());
    assert!(
        real(
            &t,
            "/home/user/.local/share/applications/gutcheck-helper.desktop"
        )
        .exists()
    );
}

#[test]
fn preview_sanitises_control_bytes() {
    let t = fixture(FLATPAK_OK);
    gutcheck(&t);
    t.write(
        "/home/user/.config/mimeapps.list",
        b"[Default Applications]\ntext/x-\x07gut\x08=gutcheck.desktop\n",
    );
    t.desktop(
        "/home/user/.local/share/applications/gutcheck.desktop",
        "[Desktop Entry]\nType=Application\nName=GutCheck\nExec=/home/user/Applications/GutCheck-0.19.0-x86_64.AppImage\n",
    );
    let out = crumbs(&t)
        .args(["uninstall", "GutCheck", "--dry-run"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(
        text.contains("    - text/x-\u{FFFD}gut\u{FFFD}=gutcheck.desktop\n"),
        "{text}"
    );
    assert!(
        !text.contains('\u{7}') && !text.contains('\u{8}'),
        "{text:?}"
    );
    // JSON keeps the raw bytes.
    let v = stdout_json(crumbs(&t).args(["uninstall", "GutCheck", "--dry-run", "--json"]));
    assert!(v.to_string().contains("\\u0007"), "{v}");
}

// ---- scan ----

fn scan_fixture(flatpak_script: &str) -> FixtureTree {
    let t = fixture(flatpak_script);
    t.write("/home/user/.cache/foo/blob", &[1u8; 3000]);
    t.write("/home/user/.cache/thumbnails/a.png", &[1u8; 2000]);
    t.write("/home/user/.local/share/Trash/files/old.txt", &[1u8; 5000]);
    t.write(
        "/home/user/.local/share/Trash/info/old.txt.trashinfo",
        b"[Trash Info]\nPath=/home/user/docs/old.txt\nDeletionDate=2026-01-01T00:00:00\n",
    );
    t.write("/home/user/.cargo/registry/cache/crate.bin", &[1u8; 7000]);
    t
}

/// Every file and symlink under the tree with its bytes.
fn tree_state(t: &FixtureTree) -> std::collections::BTreeMap<String, Vec<u8>> {
    fn walk(dir: &std::path::Path, root: &std::path::Path, out: &mut Vec<(String, Vec<u8>)>) {
        let mut entries: Vec<_> = std::fs::read_dir(dir).unwrap().flatten().collect();
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            let p = e.path();
            let rel = p.strip_prefix(root).unwrap().to_string_lossy().into_owned();
            let ft = e.file_type().unwrap();
            if ft.is_symlink() {
                let target = std::fs::read_link(&p).unwrap();
                out.push((rel, target.to_string_lossy().into_owned().into_bytes()));
            } else if ft.is_dir() {
                out.push((format!("{rel}/"), Vec::new()));
                walk(&p, root, out);
            } else {
                out.push((rel, std::fs::read(&p).unwrap()));
            }
        }
    }
    let mut v = Vec::new();
    walk(t.dir.path(), t.dir.path(), &mut v);
    v.into_iter().collect()
}

#[test]
fn scan_table_snapshot() {
    let t = scan_fixture(FLATPAK_OK);
    let out = crumbs(&t).args(["scan", "--details"]).output().unwrap();
    assert_eq!(out.status.code(), Some(0));
    let text = String::from_utf8(out.stdout).unwrap();
    let root = regex_escape(&t.dir.path().to_string_lossy());
    insta::with_settings!({ filters => vec![(root.as_str(), "[ROOT]")] }, {
        insta::assert_snapshot!(text);
    });
}

#[test]
fn scan_json_shape() {
    let t = scan_fixture(FLATPAK_OK);
    let v = stdout_json(crumbs(&t).args(["scan", "--json"]));
    assert_eq!(v["schema_version"], 1);
    assert_eq!(v["command"], "scan");
    assert!(v["errors"].as_array().unwrap().is_empty());
    let cats = v["data"]["categories"].as_array().unwrap();
    let names: Vec<_> = cats
        .iter()
        .map(|c| c["category"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        [
            "cache",
            "thumbnails",
            "trash",
            "temp",
            "browser-cache",
            "dev-cache",
            "flatpak-unused",
            "appimage-old",
            "logs"
        ]
    );
    let cache = &cats[0];
    assert_eq!(cache["skipped"], false);
    assert!(cache["error"].is_null());
    assert!(cache["total"].as_u64().unwrap() > 0);
    let item = &cache["items"][0];
    assert!(item["path"].is_string() && item["action"] == "trash");
    assert!(item.get("fingerprint").is_none());
    let dev = &cats[5];
    assert_eq!(dev["skipped"], true);
    assert_eq!(dev["items"].as_array().unwrap().len(), 0);
    assert!(dev["error"].is_null());
}

#[test]
fn scan_category_filter() {
    let t = scan_fixture(FLATPAK_OK);
    let v = stdout_json(crumbs(&t).args(["scan", "--json", "--category", "trash"]));
    let cats = v["data"]["categories"].as_array().unwrap();
    assert_eq!(cats.len(), 1);
    assert_eq!(cats[0]["category"], "trash");
    assert_eq!(cats[0]["items"].as_array().unwrap().len(), 1);
    let out = crumbs(&t)
        .args(["scan", "--category", "trash", "--category", "cache"])
        .output()
        .unwrap();
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(text.contains("cache") && text.contains("trash"), "{text}");
    assert!(!text.contains("thumbnails"), "{text}");
    crumbs(&t)
        .args(["scan", "--category", "nope"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("unknown category"));
}

#[test]
fn scan_off_category_listed_not_scanned() {
    let t = scan_fixture(FLATPAK_OK);
    let out = crumbs(&t).arg("scan").output().unwrap();
    let text = String::from_utf8(out.stdout).unwrap();
    let line = text.lines().find(|l| l.starts_with("dev-cache")).unwrap();
    assert!(
        line.ends_with("off (enable with --category or config)"),
        "{line}"
    );
    let v = stdout_json(crumbs(&t).args(["scan", "--json", "--category", "dev-cache"]));
    let dev = &v["data"]["categories"][0];
    assert_eq!(dev["skipped"], false);
    assert_eq!(dev["items"].as_array().unwrap().len(), 1);
    // Config can switch a default category off too.
    t.write(
        "/home/user/.config/crumbs/config.toml",
        b"categories_off = [\"trash\"]\n",
    );
    let v = stdout_json(crumbs(&t).args(["scan", "--json"]));
    let trash = &v["data"]["categories"][2];
    assert_eq!(trash["category"], "trash");
    assert_eq!(trash["skipped"], true);
}

#[test]
fn scan_scanner_error_exit_3() {
    let t = scan_fixture(FLATPAK_FAIL);
    let out = crumbs(&t).arg("scan").output().unwrap();
    assert_eq!(out.status.code(), Some(3));
    let text = String::from_utf8(out.stdout).unwrap();
    let line = text
        .lines()
        .find(|l| l.starts_with("flatpak-unused"))
        .unwrap();
    assert!(line.contains("boom"), "{line}");
    assert!(text.contains("reclaimable:"), "{text}");
    let v = stdout_json(crumbs(&t).args(["scan", "--json"]));
    let flat = &v["data"]["categories"][6];
    assert!(flat["error"].as_str().unwrap().contains("boom"));
    assert!(
        v["errors"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["source"] == "flatpak-unused")
    );
}

#[test]
fn scan_system_exit_2() {
    let t = scan_fixture(FLATPAK_OK);
    crumbs(&t)
        .args(["scan", "--system"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains(
            "system cleanup needs privileged support (coming in a later version)",
        ));
    let v = json_error(&t, &["scan", "--system", "--json"], 2);
    assert_eq!(v["command"], "scan");
}

#[test]
fn scan_writes_nothing() {
    let t = scan_fixture(FLATPAK_OK);
    let before = tree_state(&t);
    for args in [
        &["scan", "--details"][..],
        &["scan", "--json", "--category", "dev-cache"],
    ] {
        crumbs(&t).args(args).assert().code(0);
    }
    assert_eq!(tree_state(&t), before);
}

#[test]
fn scan_sanitises_labels() {
    let t = scan_fixture(FLATPAK_OK);
    t.write(
        "/home/user/.local/share/Trash/info/bad.trashinfo",
        b"[Trash Info]\nPath=/home/user/\x07evil\nDeletionDate=2026-01-01T00:00:00\n",
    );
    t.write("/home/user/.local/share/Trash/files/bad", b"x");
    let out = crumbs(&t).args(["scan", "--details"]).output().unwrap();
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(!text.contains('\u{7}'), "{text:?}");
    assert!(text.contains("\u{FFFD}evil"), "{text:?}");
}

// ---- clean (only ever inside the fixture) ----

/// Lists apps and runtimes; `uninstall` appends its args to
/// `<root>/flatpak-args` (the child gets no `CRUMBS_ROOT`) instead of removing anything.
const FLATPAK_RECORD: &str = "#!/bin/sh\nif [ \"$1\" = uninstall ]; then echo \"$@\" >> \"$(dirname \"$0\")/../../flatpak-args\"; exit 0; fi\nprintf 'app.xemu.xemu\\txemu\\t0.8.136\\tuser\\t34.4 MB\\n'\n";
/// The app listing works; only the runtime listing (the `flatpak-unused`
/// scanner) fails.
const FLATPAK_RUNTIME_FAIL: &str = "#!/bin/sh\ncase \"$*\" in *--runtime*) echo boom >&2; exit 1;; esac\nprintf 'app.xemu.xemu\\txemu\\t0.8.136\\tuser\\t34.4 MB\\n'\n";

/// Days `run_clean` moves crumbs' clock ahead: every fixture entry then
/// has an old ctime (which cannot be set), so mtimes decide what is old.
const CLOCK_SHIFT_DAYS: u64 = 100;

fn run_clean(t: &FixtureTree, args: &[&str]) -> (i32, String, String) {
    let out = crumbs(t)
        .env("CRUMBS_TEST_CLOCK_SHIFT_DAYS", CLOCK_SHIFT_DAYS.to_string())
        .arg("clean")
        .args(args)
        .output()
        .unwrap();
    (
        out.status.code().unwrap(),
        String::from_utf8(out.stdout).unwrap(),
        String::from_utf8(out.stderr).unwrap(),
    )
}

/// A file of the user's directly in the fixture's `/tmp`, 30 days old by
/// the clock of [`run_clean`].
fn old_temp(t: &FixtureTree, name: &str) {
    let p = t.write(&format!("/tmp/{name}"), &[1u8; 4000]);
    let old = std::time::SystemTime::now()
        + std::time::Duration::from_secs(CLOCK_SHIFT_DAYS * 86_400)
        - std::time::Duration::from_secs(30 * 86_400);
    std::fs::File::options()
        .write(true)
        .open(&p)
        .unwrap()
        .set_modified(old)
        .unwrap();
}

#[test]
fn clean_cache_trashes_and_logs() {
    let t = scan_fixture(FLATPAK_OK);
    let (code, stdout, stderr) = run_clean(&t, &["cache", "--yes"]);
    assert_eq!(code, 0, "{stdout}{stderr}");
    assert!(!real(&t, "/home/user/.cache/foo").exists());
    assert!(trash_files(&t).contains(&"foo".to_string()));
    // Only the named category.
    assert!(real(&t, "/home/user/.cache/thumbnails/a.png").exists());
    assert!(real(&t, "/home/user/.local/share/Trash/files/old.txt").exists());
    assert!(
        stdout.contains("will move 1 item (4.0 KiB) to the trash"),
        "{stdout}"
    );
    assert!(
        stdout.contains("note: space is freed when the trash is emptied (or use --permanent)"),
        "{stdout}"
    );
    assert!(stdout.contains("✓ trashed ~/.cache/foo"), "{stdout}");
    let h = history_json(&t);
    let batches = h["data"]["batches"].as_array().unwrap();
    assert_eq!(batches.len(), 1, "{h}");
    assert_eq!(batches[0]["items"], 1);
    let batch = batches[0]["batch"].as_str().unwrap();
    let last = stdout.lines().last().unwrap();
    assert!(
        last.starts_with("moved ")
            && last.ends_with(&format!(
                " to the trash · undo with: crumbs restore {batch}"
            ))
            && !last.contains("freed"),
        "{stdout}"
    );
}

#[test]
fn clean_temp_deletes_permanently_with_wording() {
    let t = scan_fixture(FLATPAK_OK);
    old_temp(&t, "old-build.log");
    let (code, stdout, _) = run_clean(&t, &["temp", "--dry-run"]);
    assert_eq!(code, 0, "{stdout}");
    assert!(stdout.contains("/tmp/old-build.log"), "{stdout}");
    assert!(
        stdout.contains("note: permanently delete (temporary files)"),
        "{stdout}"
    );
    assert!(
        stdout.contains("will permanently delete 1 item ("),
        "{stdout}"
    );
    assert!(!stdout.contains("to the trash"), "{stdout}");
    let (code, stdout, stderr) = run_clean(&t, &["temp", "--yes"]);
    assert_eq!(code, 0, "{stdout}{stderr}");
    assert!(!real(&t, "/tmp/old-build.log").exists());
    assert!(
        !trash_files(&t).contains(&"old-build.log".to_string()),
        "deleted, not trashed"
    );
    assert!(stdout.contains("✓ deleted /tmp/old-build.log"), "{stdout}");
    let last = stdout.lines().last().unwrap();
    assert!(
        last.starts_with("freed ") && last != "freed 0 B",
        "{stdout}"
    );
    assert!(!stdout.contains("undo with"), "{stdout}");
}

#[test]
fn clean_trash_empties_pairs() {
    let t = scan_fixture(FLATPAK_OK);
    let (code, stdout, stderr) = run_clean(&t, &["trash", "--yes"]);
    assert_eq!(code, 0, "{stdout}{stderr}");
    assert!(!real(&t, "/home/user/.local/share/Trash/files/old.txt").exists());
    assert!(!real(&t, "/home/user/.local/share/Trash/info/old.txt.trashinfo").exists());
    assert!(
        stdout.contains("note: permanently delete (emptying the trash)"),
        "{stdout}"
    );
    assert!(
        stdout.contains("will permanently delete 2 items ("),
        "{stdout}"
    );
    assert!(
        stdout.contains("✓ deleted ~/.local/share/Trash/info/old.txt.trashinfo"),
        "{stdout}"
    );
    let last = stdout.lines().last().unwrap();
    assert!(
        last.starts_with("freed ") && last != "freed 0 B",
        "{stdout}"
    );
    assert!(!stdout.contains("undo with"), "{stdout}");
    // The rest of the home is untouched.
    assert!(real(&t, "/home/user/.cache/foo/blob").exists());
}

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
fn clean_system_exit_2() {
    let t = scan_fixture(FLATPAK_OK);
    crumbs(&t)
        .args(["clean", "--system", "--yes"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains(
            "system cleanup needs privileged support (coming in a later version)",
        ));
    let v = json_error(&t, &["clean", "--system", "--json", "--yes"], 2);
    assert_eq!(v["command"], "clean");
    assert!(real(&t, "/home/user/.cache/foo/blob").exists());
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

/// A Firefox process in the fixture's `/proc`.
fn firefox_running(t: &FixtureTree) {
    t.symlink("/proc/79/exe", "/usr/bin/firefox");
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
fn restore_mixed_clean_batch_skips_deletes() {
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
fn scan_unknown_only_size_is_question_mark() {
    let t = scan_fixture(FLATPAK_OK);
    let out = crumbs(&t).arg("scan").output().unwrap();
    let text = String::from_utf8(out.stdout).unwrap();
    let line = text
        .lines()
        .find(|l| l.starts_with("flatpak-unused"))
        .unwrap_or_else(|| panic!("{text}"));
    let cols: Vec<&str> = line.split_whitespace().collect();
    assert_eq!(cols.get(1..3), Some(&["1", "?"][..]), "{line}");
    // A known size with unknown extras keeps its `+`.
    assert!(!text.contains("0 B+"), "{text}");
}

const EVIL_APPIMAGE: &str = "/home/user/Applications/Ev\x1b[2Jil\rX-1.0-x86_64.AppImage";

/// The fixture plus an AppImage whose file name holds ESC and CR, and its id.
fn evil_fixture() -> (FixtureTree, String) {
    let t = fixture(FLATPAK_OK);
    t.appimage(EVIL_APPIMAGE);
    let j = stdout_json(crumbs(&t).args(["apps", "list", "--json"]));
    let id = j["data"]["apps"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["id"].as_str().unwrap().to_string())
        .find(|id| id.contains('\x1b'))
        .expect("evil app listed");
    (t, id)
}

fn assert_clean(out: &std::process::Output) {
    for (name, bytes) in [("stdout", &out.stdout), ("stderr", &out.stderr)] {
        let text = String::from_utf8_lossy(bytes);
        assert!(!text.contains('\r'), "{name} has CR: {text:?}");
        assert!(!text.contains('\x1b'), "{name} has ESC: {text:?}");
    }
}

fn assert_shows_replacement(out: &std::process::Output) {
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains('\u{FFFD}'), "{text:?}");
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
fn uninstall_dry_run_sanitises_heading() {
    let (t, id) = evil_fixture();
    let out = crumbs(&t)
        .args(["uninstall", &id, "--dry-run"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    assert_clean(&out);
    assert_shows_replacement(&out);
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
