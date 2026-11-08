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
                .and(predicate::str::contains("flatpak:org.mozilla.firefox"))
                .and(predicate::str::contains(
                    "appimage:Firefox-1.0-x86_64.AppImage",
                )),
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
