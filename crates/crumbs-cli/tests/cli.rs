#![allow(clippy::unwrap_used, clippy::expect_used)]

// A release binary refuses CRUMBS_ROOT, so these tests would fail; before
// that refusal they ran against the real root and home.
#[cfg(not(debug_assertions))]
compile_error!("crumbs CLI tests must run in a debug build; they rely on CRUMBS_ROOT");

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
    // Only a test that asks for the fake privileged runner gets one.
    cmd.env_remove("CRUMBS_TEST_PRIV_RUNNER");
    cmd.env_remove("CRUMBS_ALLOW_REAL_ESCALATION");
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
        .stdout(predicate::str::contains("Escalation: none\n"))
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

/// What the fixture's filesystem allocates to an empty directory: nothing
/// on btrfs or tmpfs, a 4 KiB block on ext4 (as on CI runners). Sizes
/// count it, since deleting the directory frees it.
fn dir_block(t: &FixtureTree) -> u64 {
    use std::os::unix::fs::MetadataExt;
    let probe = t.dir.path().join(".dir-block-probe");
    std::fs::create_dir(&probe).unwrap();
    let bytes = std::fs::metadata(&probe).unwrap().blocks() * 512;
    std::fs::remove_dir(&probe).unwrap();
    bytes
}

/// A size as crumbs prints it, for the small sizes these tests use.
fn size_text(bytes: u64) -> String {
    if bytes == 0 {
        "0 B".to_string()
    } else {
        format!("{:.1} KiB", bytes as f64 / 1024.0)
    }
}

/// Rewrites what this filesystem reports to what one without directory
/// blocks reports, so a snapshot holds on both. Each row is (text marking
/// the line, bytes on a filesystem without directory blocks, directories
/// counted in it); the size keeps its column width.
fn without_dir_blocks(t: &FixtureTree, text: String, rows: &[(&str, u64, u64)]) -> String {
    let block = dir_block(t);
    if block == 0 {
        return text;
    }
    let mut out = String::new();
    for line in text.split_inclusive('\n') {
        let mut line = line.to_string();
        if let Some((_, bytes, dirs)) = rows.iter().find(|(m, _, _)| line.contains(m)) {
            let actual = size_text(bytes + dirs * block);
            // Never longer than `actual`: pad it to keep the columns.
            let wanted = format!("{:<w$}", size_text(*bytes), w = actual.len());
            line = line.replacen(&actual, &wanted, 1);
            let end = if line.ends_with('\n') { "\n" } else { "" };
            line = format!("{}{end}", line.trim_end());
        }
        out.push_str(&line);
    }
    out
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

/// The launcher goes with the AppImage file, and so do its mimeapps lines.
#[test]
fn keep_data_still_removes_mime_lines_of_removed_launcher() {
    let t = fixture(FLATPAK_OK);
    gutcheck_launcher(&t);
    t.write(
        "/home/user/.config/mimeapps.list",
        b"[Default Applications]\ntext/x-gut=appofholding-gutcheck.desktop\nimage/png=other.desktop\n",
    );
    let (code, stdout, stderr) =
        run_out(crumbs(&t).args(["uninstall", "GutCheck", "--keep-data", "--yes"]));
    assert_eq!(code, 0, "{stdout}{stderr}");
    assert!(
        !real(
            &t,
            "/home/user/.local/share/applications/appofholding-gutcheck.desktop"
        )
        .exists()
    );
    let mime = std::fs::read_to_string(real(&t, "/home/user/.config/mimeapps.list")).unwrap();
    assert!(!mime.contains("appofholding-gutcheck"), "{mime}");
    assert!(mime.contains("image/png=other.desktop"), "{mime}");
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
    if running_as_root() {
        eprintln!("skipped: the snapshot shows temp as available");
        return;
    }
    let t = scan_fixture(FLATPAK_OK);
    let out = crumbs(&t).args(["scan", "--details"]).output().unwrap();
    assert_eq!(out.status.code(), Some(0));
    let text = String::from_utf8(out.stdout).unwrap();
    let text = without_dir_blocks(
        &t,
        text,
        &[
            ("cache           1", 4 * 1024, 1),
            ("  foo", 4 * 1024, 1),
            ("thumbnails      1", 4 * 1024, 1),
            ("  thumbnails", 4 * 1024, 1),
            ("reclaimable:", 16 * 1024, 2),
        ],
    );
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
            "logs",
            "aur-build",
            "app-cache",
            "containers"
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
fn clean_exclude_leaves_out_matching_items() {
    let t = scan_fixture(FLATPAK_OK);
    // A home-relative pattern, and an absolute one under the root.
    let (code, stdout, stderr) = run_clean(
        &t,
        &[
            "cache",
            "thumbnails",
            "--exclude",
            "~/.cache/foo",
            "--dry-run",
        ],
    );
    assert_eq!(code, 0, "{stdout}{stderr}");
    assert!(!stdout.contains("~/.cache/foo"), "{stdout}");
    assert!(stdout.contains("~/.cache/thumbnails"), "{stdout}");
    let (code, stdout, stderr) = run_clean(
        &t,
        &[
            "thumbnails",
            "--exclude",
            "/home/user/.cache/thumb*",
            "--dry-run",
        ],
    );
    assert_eq!(code, 0, "{stdout}{stderr}");
    assert!(stdout.contains("nothing to clean"), "{stdout}");
}

#[test]
fn clean_exclude_must_be_absolute() {
    let t = scan_fixture(FLATPAK_OK);
    let (code, stdout, stderr) = run_clean(&t, &["cache", "--exclude", "foo", "--dry-run"]);
    assert_eq!(code, 2, "{stdout}{stderr}");
    assert!(
        stderr.contains("--exclude 'foo' must start with / or ~/"),
        "{stderr}"
    );
}

#[test]
fn clean_preview_warns_when_the_disk_is_nearly_full() {
    let t = scan_fixture(FLATPAK_OK);
    let out = crumbs(&t)
        .env("CRUMBS_TEST_CLOCK_SHIFT_DAYS", CLOCK_SHIFT_DAYS.to_string())
        .env("CRUMBS_TEST_FS_SPACE", "1048576/104857600")
        .args(["clean", "cache", "--dry-run"])
        .output()
        .unwrap();
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert_eq!(out.status.code(), Some(0), "{stdout}");
    assert!(
        stdout.contains("note: the disk is 99% full (1.0 MiB free): moving to the trash"),
        "{stdout}"
    );
}

/// A flatpak 1.x whose `uninstall --unused` without `-y` lists one unused
/// runtime and declines, as flatpak does when stdout is not a terminal.
/// Every `uninstall` it gets is recorded in `<root>/flatpak-args`.
const FLATPAK_PROBED: &str = "#!/bin/sh\nroot=\"$(dirname \"$0\")/../..\"\ncase \"$*\" in\n--version) echo 'Flatpak 1.18.4' ;;\nuninstall*) echo \"$@\" >> \"$root/flatpak-args\"; case \"$*\" in *-y*) ;; *) printf ' 1.     org.gnome.Platform   45   r\\nProceed with these changes to the user installation? [Y/n]: n\\n'; exit 1 ;; esac ;;\n*runtime*) echo org.gnome.Platform/x86_64/45 ;;\n*) printf 'app.xemu.xemu\\txemu\\t0.8.136\\tuser\\t34.4 MB\\n' ;;\nesac\n";

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
        stdout.contains(&format!(
            "will move 1 item ({}) to the trash",
            size_text(4 * 1024 + dir_block(&t))
        )),
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
    if running_as_root() {
        eprintln!("skipped: temp cleanup is not offered as root");
        return;
    }
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

// ---- full uninstall with one privileged batch (only ever inside the fixture) ----

/// A fake `pacman`: the simulation (`-Rs --print ...`) prints firefox and
/// two dependencies; the removal (`-Rn --noconfirm <names>`) logs its
/// arguments to `<root>/pacman-args` and deletes those packages' database
/// entries. It runs as the test user, inside the fixture.
const PACMAN_OK: &str = "#!/bin/sh\nroot=\"$(dirname \"$0\")/../..\"\ncase \"$1\" in\n-Rs) printf 'firefox 159.0.1-1\\ngn 0.2385.1-1\\nlibjuice 1.7.3-1\\n' ;;\n-Rn) echo \"$@\" >> \"$root/pacman-args\"; shift 2; for p in \"$@\"; do rm -rf \"$root\"/var/lib/pacman/local/\"$p\"-*; done ;;\n*) exit 9 ;;\nesac\n";
/// The simulation refuses, with an escape byte in its message.
const PACMAN_REFUSE: &str = "#!/bin/sh\nprintf ':: removing firefox breaks dependency \\033[1mfirefox\\033[0m required by foo\\n'\necho 'error: failed to prepare transaction (could not satisfy dependencies)' >&2\nexit 1\n";
/// Lists the fixture's Flatpak apps; `uninstall` logs its arguments to
/// `<root>/flatpak-args` instead of removing anything.
const FLATPAK_APPS_RECORD: &str = "#!/bin/sh\nif [ \"$1\" = uninstall ]; then echo \"$@\" >> \"$(dirname \"$0\")/../../flatpak-args\"; exit 0; fi\nprintf 'app.xemu.xemu\\txemu\\t0.8.136\\tuser\\t34.4 MB\\ncom.dosbox_x.DOSBox-X\\tDOSBox-X\\t2026.10.01\\tsystem\\t51.4 MB\\n'\n";

fn pacman_fixture(pacman: &str) -> FixtureTree {
    let t = fixture(FLATPAK_OK);
    t.pacman_pkg("gn", "0.2385.1-1", Some(1_000), &["usr/bin/gn"]);
    t.pacman_pkg("libjuice", "1.7.3-1", Some(1_000), &["usr/lib/libjuice.so"]);
    t.write_exec("/usr/bin/pacman", pacman.as_bytes());
    t.write("/home/user/.cache/firefox/blob", &[b'x'; 4000]);
    t
}

fn run_out(cmd: &mut Command) -> (i32, String, String) {
    let out = cmd.output().unwrap();
    (
        out.status.code().unwrap(),
        String::from_utf8(out.stdout).unwrap(),
        String::from_utf8(out.stderr).unwrap(),
    )
}

fn pkg_present(t: &FixtureTree, dir: &str) -> bool {
    real(t, &format!("/var/lib/pacman/local/{dir}/desc")).exists()
}

const MANUAL_PACMAN: &str = "sudo '/usr/bin/pacman' '-Rn' '--noconfirm' 'firefox' 'gn' 'libjuice'";

#[test]
fn uninstall_pacman_dry_run_shows_also_removed() {
    let t = pacman_fixture(PACMAN_OK);
    let (code, stdout, stderr) = run_out(crumbs(&t).args(["uninstall", "firefox", "--dry-run"]));
    assert_eq!(code, 0, "{stdout}{stderr}");
    let row = stdout
        .lines()
        .find(|l| l.starts_with("remove"))
        .unwrap_or_else(|| panic!("{stdout}"));
    assert!(
        row.contains("—") && row.contains("pacman: firefox 159.0.1-1"),
        "{row}"
    );
    assert!(
        stdout.contains("also removed: gn 0.2385.1-1, libjuice 1.7.3-1"),
        "{stdout}"
    );
    assert!(stdout.contains("~/.cache/firefox"), "{stdout}");
    assert!(
        stdout.contains("note: these steps run with administrator rights; you will be asked once"),
        "{stdout}"
    );
    assert!(
        stdout.contains("note: this cannot be undone by crumbs"),
        "{stdout}"
    );
    assert!(!stdout.contains("leftovers only"), "{stdout}");
    assert!(pkg_present(&t, "firefox-159.0.1-1"));
    assert!(real(&t, "/home/user/.cache/firefox/blob").exists());
    assert!(!real(&t, "/pacman-args").exists());
    // JSON: the package action leads the plan; leftovers depend on it.
    let v = stdout_json(crumbs(&t).args(["uninstall", "firefox", "--dry-run", "--json"]));
    let actions = v["data"]["plans"][0]["actions"].as_array().unwrap();
    assert_eq!(actions[0]["action"]["type"], "package-remove", "{v}");
    assert_eq!(
        actions[0]["action"]["args"],
        serde_json::json!(["-Rn", "--noconfirm", "firefox", "gn", "libjuice"])
    );
    assert!(actions.len() > 1, "{v}");
    assert!(actions[1..].iter().all(|a| a["depends_on"] == 0), "{v}");
}

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

/// Whether the tests run as root (as in the distro containers): escalation
/// detection then reports running as root instead of run0/pkexec/sudo.
fn running_as_root() -> bool {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata("/proc/self").is_ok_and(|m| m.uid() == 0)
}

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

/// Like [`PACMAN_OK`], but any other package simulates as just itself.
const PACMAN_MULTI: &str = "#!/bin/sh\nroot=\"$(dirname \"$0\")/../..\"\ncase \"$1\" in\n-Rs) if [ \"$5\" = firefox ]; then printf 'firefox 159.0.1-1\\ngn 0.2385.1-1\\nlibjuice 1.7.3-1\\n'; else printf '%s 1.0-1\\n' \"$5\"; fi ;;\n-Rn) echo \"$@\" >> \"$root/pacman-args\"; shift 2; for p in \"$@\"; do rm -rf \"$root\"/var/lib/pacman/local/\"$p\"-*; done ;;\n*) exit 9 ;;\nesac\n";

fn two_pacman_apps() -> FixtureTree {
    let t = pacman_fixture(PACMAN_MULTI);
    t.pacman_pkg(
        "kate",
        "1.0-1",
        Some(1_000),
        &["usr/bin/kate", "usr/share/applications/kate.desktop"],
    );
    t.write_exec("/usr/bin/kate", b"");
    t.desktop(
        "/usr/share/applications/kate.desktop",
        "[Desktop Entry]\nType=Application\nName=Kate\nExec=/usr/bin/kate\n",
    );
    t.write("/home/user/.cache/kate/blob", b"k");
    t
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
fn preview_shows_root_command_lines() {
    let t = fixture(FLATPAK_APPS_RECORD);
    let (code, stdout, _) = run_out(crumbs(&t).args(["uninstall", "DOSBox-X", "--dry-run"]));
    assert_eq!(code, 0, "{stdout}");
    assert!(
        stdout.contains(
            "    - as root: '/usr/bin/flatpak' 'uninstall' '--system' '-y' '--noninteractive' 'com.dosbox_x.DOSBox-X'"
        ),
        "{stdout}"
    );
    let t = pacman_fixture(PACMAN_OK);
    let (code, stdout, _) = run_out(crumbs(&t).args(["uninstall", "firefox", "--dry-run"]));
    assert_eq!(code, 0, "{stdout}");
    assert!(
        stdout.contains(
            "    - as root: '/usr/bin/pacman' '-Rn' '--noconfirm' 'firefox' 'gn' 'libjuice'"
        ),
        "{stdout}"
    );
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

// ---- system categories (`--system`) ----

const SYS_ORPHANS: [&str; 7] = [
    "orph1", "orph2", "orph3", "orph4", "orph5", "orph6", "orph7",
];

/// A fixture with every system tool as a recording script. Each one logs
/// its arguments to `<root>/<tool>-args` when it changes something. The
/// journal query first runs `JOURNAL_HOOK` (empty unless a test sets it).
fn system_fixture(journal_hook: &str) -> FixtureTree {
    let t = FixtureTree::new();
    t.write_exec(
        "/usr/bin/paccache",
        b"#!/bin/sh\nroot=\"$(dirname \"$0\")/../..\"\ncase \"$1\" in\n-dk2) printf '==> finished dry run: 1 candidates (disk space saved: 86.17 MiB)\\n' ;;\n-rk2) echo \"$@\" >> \"$root/paccache-args\" ;;\n*) exit 9 ;;\nesac\n",
    );
    t.write_exec(
        "/usr/bin/pacman",
        b"#!/bin/sh\nroot=\"$(dirname \"$0\")/../..\"\ncase \"$1\" in\n-Qdtq) printf 'orph1\\norph2\\norph3\\norph4\\norph5\\norph6\\norph7\\n' ;;\n-Rs) shift 4; for p in \"$@\"; do printf '%s 1.0-1\\n' \"$p\"; done ;;\n-Rn) echo \"$@\" >> \"$root/pacman-args\" ;;\n*) exit 9 ;;\nesac\n",
    );
    let journalctl = format!(
        "#!/bin/sh\nroot=\"$(dirname \"$0\")/../..\"\ncase \"$1\" in\n--disk-usage) {journal_hook}\nprintf 'Archived and active journals take up 719.1M in the file system.\\n' ;;\n--vacuum-size=500M) echo \"$@\" >> \"$root/journal-args\" ;;\n*) exit 9 ;;\nesac\n"
    );
    t.write_exec("/usr/bin/journalctl", journalctl.as_bytes());
    t.write_exec(
        "/usr/bin/systemd-tmpfiles",
        b"#!/bin/sh\necho \"$@\" >> \"$(dirname \"$0\")/../../tmpfiles-args\"\n",
    );
    t.write_exec(
        "/usr/bin/flatpak",
        b"#!/bin/sh\nroot=\"$(dirname \"$0\")/../..\"\nif [ \"$1\" = list ]; then case \"$*\" in *--runtime*) echo org.x.Platform/x86_64/23.08;; esac; exit 0; fi\n[ \"$1\" = --version ] && exit 0\necho \"$@\" >> \"$root/flatpak-args\"\n",
    );
    // `rm` logs, then removes the root-joined path the batch passes.
    t.write_exec(
        "/usr/bin/rm",
        b"#!/bin/sh\necho \"$@\" >> \"$(dirname \"$0\")/../../rm-args\"\nexec /usr/bin/rm \"$@\"\n",
    );
    t.mkdir("var/lib/systemd/coredump");
    let mut cur = t.dir.path().to_path_buf();
    for c in ["", "var", "lib", "systemd", "coredump"] {
        cur.push(c);
        std::fs::set_permissions(&cur, std::os::unix::fs::PermissionsExt::from_mode(0o755))
            .unwrap();
    }
    t.write(
        "/var/lib/systemd/coredump/core.foo.1000.abc.1.zst",
        &[7u8; 9000],
    );
    t
}

const CORE: &str = "/var/lib/systemd/coredump/core.foo.1000.abc.1.zst";

fn sh_runner(t: &FixtureTree) -> Command {
    let mut cmd = crumbs(t);
    cmd.env("CRUMBS_TEST_PRIV_RUNNER", "sh");
    cmd
}

fn recorded(t: &FixtureTree, file: &str) -> Option<String> {
    std::fs::read_to_string(real(t, file)).ok()
}

#[test]
fn scan_system_table() {
    let t = system_fixture("");
    let (code, stdout, stderr) = run_out(crumbs(&t).args(["scan", "--system"]));
    assert_eq!(code, 0, "{stdout}{stderr}");
    for c in [
        "pkg-cache",
        "orphan-pkgs",
        "journal",
        "coredumps",
        "system-temp",
        "flatpak-unused-system",
    ] {
        assert!(stdout.lines().any(|l| l.starts_with(c)), "{c}: {stdout}");
    }
    assert!(
        !stdout.lines().any(|l| l.starts_with("thumbnails")),
        "{stdout}"
    );
    assert!(!stdout.lines().any(|l| l.starts_with("trash")), "{stdout}");
    let v = stdout_json(crumbs(&t).args(["scan", "--system", "--json"]));
    assert_eq!(v["data"]["categories"].as_array().unwrap().len(), 6, "{v}");
    // A scan changes nothing and needs no privileges.
    for f in [
        "paccache-args",
        "pacman-args",
        "journal-args",
        "tmpfiles-args",
        "rm-args",
    ] {
        assert!(recorded(&t, f).is_none(), "{f}");
    }
    assert!(real(&t, CORE).exists());
}

#[test]
fn scan_system_unavailable_shows_reason() {
    let t = FixtureTree::new();
    let (code, stdout, stderr) = run_out(crumbs(&t).args(["scan", "--system"]));
    assert_eq!(code, 0, "{stdout}{stderr}");
    let line = stdout.lines().find(|l| l.starts_with("pkg-cache")).unwrap();
    assert!(
        line.contains("unavailable (pacman, apt-get or dnf not installed)"),
        "{stdout}"
    );
    assert!(
        stdout.lines().any(
            |l| l.starts_with("coredumps") && l.contains("unavailable (no coredump directory)")
        ),
        "{stdout}"
    );
    // JSON keeps its own field and has no human text.
    let v = stdout_json(crumbs(&t).args(["scan", "--system", "--json"]));
    let first = &v["data"]["categories"][0];
    assert_eq!(
        first["unavailable"], "pacman, apt-get or dnf not installed",
        "{v}"
    );
}

#[test]
fn scan_user_and_system_conflict_exit_2() {
    let t = system_fixture("");
    crumbs(&t)
        .args(["scan", "--user", "--system"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("use either --user or --system"));
    let v = json_error(&t, &["scan", "--user", "--system", "--json"], 2);
    assert_eq!(v["command"], "scan");
    crumbs(&t)
        .args(["clean", "--user", "--system", "--dry-run"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("use either --user or --system"));
    let v = json_error(&t, &["clean", "--user", "--system", "--json", "--yes"], 2);
    assert_eq!(v["command"], "clean");
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

#[test]
fn clean_system_runs_one_batch() {
    let t = system_fixture("");
    let (code, stdout, stderr) = run_out(sh_runner(&t).args(["clean", "--system", "--yes"]));
    assert_eq!(code, 0, "{stdout}{stderr}");
    assert_eq!(
        stderr.matches("crumbs-test: privileged batch").count(),
        1,
        "{stderr}"
    );
    assert_eq!(recorded(&t, "/paccache-args").as_deref(), Some("-rk2\n"));
    assert_eq!(
        recorded(&t, "/journal-args").as_deref(),
        Some("--vacuum-size=500M\n")
    );
    assert_eq!(
        recorded(&t, "/tmpfiles-args").as_deref(),
        Some("--clean --prefix=/tmp --prefix=/var/tmp\n")
    );
    assert!(!real(&t, CORE).exists());
    assert!(recorded(&t, "/rm-args").unwrap().contains("core.foo"));
    // Opt-in categories did not run.
    assert!(recorded(&t, "/pacman-args").is_none());
    assert!(recorded(&t, "/flatpak-args").is_none());
    let h = history_json(&t).to_string();
    assert!(h.contains("privileged-command"), "{h}");
}

#[test]
fn clean_system_orphan_pkgs_opt_in() {
    let t = system_fixture("");
    // Naming the category implies --system, and opts in.
    let (code, stdout, stderr) = run_out(crumbs(&t).args(["clean", "orphan-pkgs", "--dry-run"]));
    assert_eq!(code, 0, "{stdout}{stderr}");
    // The preview shows the whole closure, not the scanner's short note.
    let all = SYS_ORPHANS.map(|n| format!("'{n}'")).join(" ");
    assert!(
        stdout.contains(&format!(
            "as root: '/usr/bin/pacman' '-Rn' '--noconfirm' {all}"
        )),
        "{stdout}"
    );
    assert!(!stdout.contains("paccache"), "{stdout}");
    let (code, stdout, stderr) = run_out(sh_runner(&t).args(["clean", "orphan-pkgs", "--yes"]));
    assert_eq!(code, 0, "{stdout}{stderr}");
    assert_eq!(
        recorded(&t, "/pacman-args").unwrap(),
        format!("-Rn --noconfirm {}\n", SYS_ORPHANS.join(" "))
    );
    assert!(recorded(&t, "/paccache-args").is_none());
    assert!(real(&t, CORE).exists());
    // Without naming it, `--system` leaves it out (the first test above
    // already shows the preview); the flatpak one is opt-in too.
    let (code, stdout, stderr) =
        run_out(sh_runner(&t).args(["clean", "flatpak-unused-system", "--yes"]));
    assert_eq!(code, 0, "{stdout}{stderr}");
    assert_eq!(
        recorded(&t, "/flatpak-args").unwrap(),
        "uninstall --system --unused -y --noninteractive\n"
    );
}

#[test]
fn clean_system_escalation_denied_exit_5() {
    let t = system_fixture("");
    let manual = format!("sudo '/usr/bin/rm' '-f' '--' '{CORE}'");
    let (code, stdout, stderr) = run_out(
        crumbs(&t)
            .env("CRUMBS_TEST_PRIV_RUNNER", "deny")
            .args(["clean", "--system", "--yes"]),
    );
    assert_eq!(code, 5, "{stdout}{stderr}");
    assert!(
        stderr
            .contains("administrator rights were not granted; nothing that needs them was changed"),
        "{stderr}"
    );
    assert!(stderr.contains(&manual), "{stderr}");
    assert!(
        stderr.contains("sudo '/usr/bin/paccache' '-rk2'"),
        "{stderr}"
    );
    assert!(real(&t, CORE).exists());
    assert!(recorded(&t, "/paccache-args").is_none());
    // No tree delete among them: no mount note.
    assert!(
        !stderr.contains("check that nothing is mounted"),
        "{stderr}"
    );
    // No fake runner at all: nothing to escalate with.
    let (code, stdout, stderr) = run_out(crumbs(&t).args(["clean", "--system", "--yes"]));
    assert_eq!(code, 5, "{stdout}{stderr}");
    assert!(
        stderr.contains("no way to get administrator rights"),
        "{stderr}"
    );
    assert!(stderr.contains(&manual), "{stderr}");
    let v = json_error(&t, &["clean", "--system", "--json", "--yes"], 5);
    assert_eq!(
        v["data"]["manual_commands"],
        serde_json::json!([
            "sudo '/usr/bin/paccache' '-rk2'",
            "sudo '/usr/bin/journalctl' '--vacuum-size=500M'",
            manual,
            "sudo '/usr/bin/systemd-tmpfiles' '--clean' '--prefix=/tmp' '--prefix=/var/tmp'",
        ]),
        "{v}"
    );
    assert!(real(&t, CORE).exists());
}

/// Swaps the coredump for a symlink to `<root>/precious` after the scan
/// and confirmation, before anything executes (a debug-only test hook).
fn swap_hook() -> String {
    format!("rm -f \"$CRUMBS_ROOT{CORE}\"; ln -s \"$CRUMBS_ROOT/precious\" \"$CRUMBS_ROOT{CORE}\"")
}

#[test]
fn clean_system_coredump_swapped_refused() {
    let t = system_fixture("");
    t.write("/precious", b"keep me");
    let (code, stdout, stderr) = run_out(
        sh_runner(&t)
            .env("CRUMBS_TEST_BEFORE_EXECUTE", swap_hook())
            .args(["clean", "--system", "--yes"]),
    );
    assert_eq!(code, 3, "{stdout}{stderr}");
    let line = stdout
        .lines()
        .find(|l| l.starts_with('✗') && l.contains("core.foo"))
        .unwrap_or_else(|| panic!("{stdout}"));
    assert!(
        line.contains("symlink") || line.contains("changed"),
        "{line}"
    );
    // No rm step ran, and nothing was deleted through the link.
    assert!(
        recorded(&t, "/rm-args").is_none(),
        "{:?}",
        recorded(&t, "/rm-args")
    );
    assert!(
        std::fs::symlink_metadata(real(&t, CORE))
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(std::fs::read(real(&t, "/precious")).unwrap(), b"keep me");
    // The other items in the batch still ran.
    assert_eq!(recorded(&t, "/paccache-args").as_deref(), Some("-rk2\n"));
}

#[test]
fn clean_system_manual_commands_exclude_refused_actions() {
    let t = system_fixture("");
    t.write("/precious", b"keep me");
    let out = crumbs(&t)
        .env("CRUMBS_TEST_PRIV_RUNNER", "deny")
        .env("CRUMBS_TEST_BEFORE_EXECUTE", swap_hook())
        .args(["clean", "--system", "--yes", "--json"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(5));
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    // Only what failed for lack of rights; the swapped coredump was refused
    // by the check and is not offered as a command to run by hand.
    assert_eq!(
        v["data"]["manual_commands"],
        serde_json::json!([
            "sudo '/usr/bin/paccache' '-rk2'",
            "sudo '/usr/bin/journalctl' '--vacuum-size=500M'",
            "sudo '/usr/bin/systemd-tmpfiles' '--clean' '--prefix=/tmp' '--prefix=/var/tmp'",
        ]),
        "{v}"
    );
    assert_eq!(std::fs::read(real(&t, "/precious")).unwrap(), b"keep me");
}

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
fn clean_default_still_user_only() {
    let t = system_fixture("");
    t.write("/home/user/.cache/foo/blob", &[1u8; 3000]);
    // No flag: user categories only, and nothing needs privileges.
    let (code, stdout, stderr) = run_clean(&t, &["--yes"]);
    assert_eq!(code, 0, "{stdout}{stderr}");
    assert!(!real(&t, "/home/user/.cache/foo").exists());
    assert!(real(&t, CORE).exists());
    for f in ["paccache-args", "journal-args", "tmpfiles-args", "rm-args"] {
        assert!(recorded(&t, f).is_none(), "{f}");
    }
    let (_, stdout, _) = run_out(crumbs(&t).args(["scan"]));
    for c in [
        "pkg-cache",
        "orphan-pkgs",
        "journal",
        "coredumps",
        "system-temp",
    ] {
        assert!(!stdout.lines().any(|l| l.starts_with(c)), "{c}: {stdout}");
    }
    assert!(stdout.lines().any(|l| l.starts_with("cache")), "{stdout}");
}

#[test]
fn clean_system_batch_stopped_part_way_exit_3() {
    let t = system_fixture("");
    // The batch's steps: paccache, journalctl, rm (the core dump), tmpfiles;
    // the root shell ends during the third.
    let (code, stdout, stderr) = run_out(
        crumbs(&t)
            .env("CRUMBS_TEST_PRIV_RUNNER", "partial:2")
            .args(["clean", "--system", "--yes"]),
    );
    assert_eq!(code, 3, "{stdout}{stderr}");
    assert!(
        !stderr.contains("nothing that needs them was changed"),
        "{stderr}"
    );
    assert!(
        stderr.contains("error: the privileged run stopped before it finished"),
        "{stderr}"
    );
    assert!(!stderr.contains("to do it yourself"), "{stderr}");
    assert!(
        stdout.contains("the privileged run stopped during this step; it may have partly run"),
        "{stdout}"
    );
    assert!(
        stdout.contains("not run: the privileged run stopped"),
        "{stdout}"
    );
    assert_eq!(recorded(&t, "/paccache-args").as_deref(), Some("-rk2\n"));
    assert!(recorded(&t, "/journal-args").is_some());
    assert!(recorded(&t, "/tmpfiles-args").is_none());
    assert!(real(&t, CORE).exists());
    // The history records what ran and what did not.
    let h = history_json(&t).to_string();
    assert!(h.contains("paccache"), "{h}");
    assert!(h.contains("it may have partly run"), "{h}");
    assert!(h.contains("not run: the privileged run stopped"), "{h}");
    // JSON: the same exit, a cli error, and no manual commands.
    let t = system_fixture("");
    let out = crumbs(&t)
        .env("CRUMBS_TEST_PRIV_RUNNER", "partial:1")
        .args(["clean", "--system", "--yes", "--json"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(3));
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(v["data"].get("manual_commands").is_none(), "{v}");
    let results = &v["data"]["report"][0]["results"];
    assert_eq!(results[0]["ok"], true, "{v}");
    assert_eq!(results[1]["ok"], false, "{v}");
    let errors = v["errors"].as_array().unwrap();
    assert!(
        errors.iter().any(|e| e["source"] == "cli"
            && e["message"]
                .as_str()
                .unwrap()
                .starts_with("the privileged run stopped")),
        "{v}"
    );
}

#[test]
fn clean_user_and_system_categories_in_one_run() {
    let t = system_fixture("");
    t.write("/home/user/.cache/foo/blob", &[b'y'; 9000]);
    let (code, stdout, stderr) =
        run_out(sh_runner(&t).args(["clean", "cache", "pkg-cache", "--yes"]));
    assert_eq!(code, 0, "{stdout}{stderr}");
    assert_eq!(
        stderr.matches("crumbs-test: privileged batch").count(),
        1,
        "{stderr}"
    );
    assert_eq!(recorded(&t, "/paccache-args").as_deref(), Some("-rk2\n"));
    assert!(!real(&t, "/home/user/.cache/foo").exists());
    assert!(!trash_files(&t).is_empty(), "the cache went to the trash");
    // Unnamed system categories did not run.
    assert!(recorded(&t, "/journal-args").is_none());
    assert!(real(&t, CORE).exists());
}

#[test]
fn clean_user_and_system_without_rights_changes_nothing_exit_5() {
    let t = system_fixture("");
    t.write("/home/user/.cache/foo/blob", &[b'y'; 9000]);
    let (code, stdout, stderr) = run_out(crumbs(&t).args(["clean", "cache", "pkg-cache", "--yes"]));
    assert_eq!(code, 5, "{stdout}{stderr}");
    assert!(
        stderr.contains("no way to get administrator rights"),
        "{stderr}"
    );
    assert!(
        stderr.contains("sudo '/usr/bin/paccache' '-rk2'"),
        "{stderr}"
    );
    // The whole run stops before anything executes: user items too.
    assert!(real(&t, "/home/user/.cache/foo/blob").exists());
    assert!(trash_files(&t).is_empty());
    assert!(recorded(&t, "/paccache-args").is_none());
}

// ---- system leftovers, unmanaged removal, `orphans --system` ----

/// Makes `/`, `/etc` and `/opt` pass the system-tree checks (this user
/// stands in for root) and installs a logging `rm` for the root batch.
fn sys_ready(t: &FixtureTree) {
    t.mkdir("etc");
    t.mkdir("opt");
    for rel in ["", "etc", "opt"] {
        std::fs::set_permissions(
            t.dir.path().join(rel),
            std::os::unix::fs::PermissionsExt::from_mode(0o755),
        )
        .unwrap();
    }
    t.write_exec(
        "/usr/bin/rm",
        b"#!/bin/sh\necho \"$@\" >> \"$(dirname \"$0\")/../../rm-args\"\nexec /usr/bin/rm \"$@\"\n",
    );
    t.write_exec("/usr/bin/sh", b"#!/bin/sh\nexec /bin/sh \"$@\"\n");
    // The guard compares mount namespaces with the trusted `readlink`.
    t.write_exec(
        "/usr/bin/readlink",
        b"#!/bin/sh\nexec /usr/bin/readlink \"$@\"\n",
    );
    // The root batch refuses a tree with a mount point inside.
    t.write(
        "/proc/self/mountinfo",
        b"36 35 98:0 /mnt1 / rw,noatime master:1 - ext3 /dev/root rw\n",
    );
}

/// A package `inky` that owns only its launcher, with unowned `/opt/inky`
/// and `/etc/inky` beside it.
fn inky_pkg_fixture() -> FixtureTree {
    let t = FixtureTree::new();
    t.os_release("manjaro", "arch", "Manjaro Linux");
    t.pacman_pkg("coreutils", "9.5-1", Some(2_000_000), &["usr/bin/ls"]);
    t.pacman_pkg("inky", "1.0-1", Some(1_000), &["usr/bin/inky"]);
    t.write_exec("/usr/bin/inky", b"");
    t.write("/opt/inky/bin/inky", b"x");
    t.write("/etc/inky/inky.conf", b"x");
    sys_ready(&t);
    t
}

/// An unmanaged app `inky` (an unowned launcher running `/opt/inky/inky`)
/// with `/etc/inky` beside it; the package database is complete.
fn inky_unmanaged_fixture() -> FixtureTree {
    let t = FixtureTree::new();
    t.os_release("manjaro", "arch", "Manjaro Linux");
    t.pacman_pkg("coreutils", "9.5-1", Some(2_000_000), &["usr/bin/ls"]);
    t.write_exec("/opt/inky/inky", b"");
    t.write("/etc/inky/inky.conf", b"x");
    t.desktop(
        "/usr/share/applications/inky.desktop",
        "[Desktop Entry]\nType=Application\nName=Inky\nExec=/opt/inky/inky\n",
    );
    sys_ready(&t);
    t
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

fn old_app_fixture() -> FixtureTree {
    let t = inky_unmanaged_fixture();
    t.write("/opt/old-app/bin/x", b"x");
    t.write("/etc/old-app/x.conf", b"x");
    t
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

#[test]
fn version_flag_prints_name_and_version() {
    Command::cargo_bin("crumbs")
        .unwrap()
        .arg("--version")
        .assert()
        .success()
        .stdout(format!("crumbs {}\n", env!("CARGO_PKG_VERSION")));
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
