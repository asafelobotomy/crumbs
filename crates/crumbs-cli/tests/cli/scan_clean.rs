use super::*;

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
        "snapshots",
    ] {
        assert!(stdout.lines().any(|l| l.starts_with(c)), "{c}: {stdout}");
    }
    assert!(
        !stdout.lines().any(|l| l.starts_with("thumbnails")),
        "{stdout}"
    );
    assert!(!stdout.lines().any(|l| l.starts_with("trash")), "{stdout}");
    let v = stdout_json(crumbs(&t).args(["scan", "--system", "--json"]));
    assert_eq!(v["data"]["categories"].as_array().unwrap().len(), 7, "{v}");
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
