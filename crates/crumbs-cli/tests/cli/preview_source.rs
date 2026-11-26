use super::*;

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
            "containers",
            "build-output"
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
