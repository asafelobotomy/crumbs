use super::*;

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
fn clean_snapshots_only_when_named() {
    let t = system_fixture("");
    t.write_exec("/usr/bin/timeshift", b"#!/bin/sh\n");
    t.write("/etc/timeshift/timeshift.json", b"{}");
    // Not in a plain system clean.
    let (code, stdout, stderr) = run_out(crumbs(&t).args(["clean", "--system", "--dry-run"]));
    assert_eq!(code, 0, "{stdout}{stderr}");
    assert!(!stdout.contains("Timeshift"), "{stdout}");
    // Named: planned, as a root step that keeps the newest 3.
    let (code, stdout, stderr) = run_out(crumbs(&t).args(["clean", "snapshots", "--dry-run"]));
    assert_eq!(code, 0, "{stdout}{stderr}");
    assert!(
        stdout.contains("Timeshift snapshots beyond the newest 3"),
        "{stdout}"
    );
    assert!(stdout.contains("will run 1 command"), "{stdout}");
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
