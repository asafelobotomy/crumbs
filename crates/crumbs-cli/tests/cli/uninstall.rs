use super::*;

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
