use super::*;

#[test]
fn aur_variant_keeps_the_plain_name() {
    let t = FixtureTree::new();
    t.mkdir("/home/user/.cache/paru");
    t.mkdir("/home/user/.config/paru");
    t.mkdir("/home/user/.config/kite");
    let fs = orphans(
        &t,
        vec![pacman("paru-bin", &[]), pacman("kite-git", &[])],
        true,
    );
    assert!(fs.is_empty(), "{fs:#?}");
    // And it is the variant's leftover on uninstall.
    let r = Run::new(&t, vec![pacman("paru-bin", &[])]);
    let fs = r.find(0, true);
    assert_eq!(must(&fs, ".cache/paru").confidence, Confidence::High);
}

#[test]
fn data_named_after_an_installed_program_is_not_orphan() {
    let t = FixtureTree::new();
    t.write_exec("/usr/bin/gh", b"");
    t.write_exec("/usr/bin/cargo", b"");
    t.write_exec("/home/user/.local/bin/mytool", b"");
    t.mkdir("/home/user/.config/gh");
    t.mkdir("/home/user/.cargo/registry");
    t.mkdir("/home/user/.local/share/mytool");
    t.mkdir("/home/user/.config/gone-tool");
    let fs = orphans(&t, Vec::new(), true);
    assert!(at(&fs, ".config/gh").is_none(), "{fs:#?}");
    assert!(at(&fs, ".cargo").is_none(), "{fs:#?}");
    assert!(at(&fs, ".local/share/mytool").is_none(), "{fs:#?}");
    assert!(at(&fs, ".config/gone-tool").is_some(), "{fs:#?}");
}

#[test]
fn appofholding_library_apps_keep_their_data() {
    let t = FixtureTree::new();
    t.write("/home/user/.local/share/appofholding/db.sqlite", b"");
    t.write(
        "/home/user/Applications/GutCheck-0.28.1-x86_64.AppImage",
        b"",
    );
    t.write("/home/user/Applications/Grok_Bot_1.1.0.AppImage", b"");
    t.mkdir("/home/user/Applications/qbit-plugin-dl");
    t.mkdir("/home/user/.config/GutCheck");
    t.mkdir("/home/user/.config/Grok Bot");
    t.mkdir("/home/user/.config/qbit-plugin-dl");
    t.mkdir("/home/user/.config/gone-app");
    // Launchers: one for a library AppImage, one pointing at a file that
    // is gone (broken, whoever made it).
    t.desktop(
        "/home/user/.local/share/applications/gutcheck.desktop",
        &format!(
            "[Desktop Entry]\nType=Application\nName=GutCheck\nExec={HOME}/Applications/GutCheck-0.28.1-x86_64.AppImage\n"
        ),
    );
    t.desktop(
        "/home/user/.config/autostart/grok-bot.desktop",
        &format!("[Desktop Entry]\nType=Application\nExec={HOME}/Applications/grok-bot\n"),
    );
    let fs = orphans(&t, Vec::new(), true);
    let paths: Vec<_> = fs.iter().map(|f| f.path.clone()).collect();
    assert!(at(&fs, ".config/GutCheck").is_none(), "{paths:#?}");
    assert!(at(&fs, ".config/Grok Bot").is_none(), "{paths:#?}");
    assert!(at(&fs, ".config/qbit-plugin-dl").is_none(), "{paths:#?}");
    assert!(
        at(&fs, ".local/share/applications/gutcheck.desktop").is_none(),
        "{paths:#?}"
    );
    assert!(at(&fs, ".config/gone-app").is_some(), "{paths:#?}");
    let broken = must(&fs, ".config/autostart/grok-bot.desktop");
    assert_eq!(broken.confidence, Confidence::Medium);
    // Without appOFholding's database they are not its library.
    std::fs::remove_file(
        t.dir
            .path()
            .join("home/user/.local/share/appofholding/db.sqlite"),
    )
    .unwrap();
    let fs = orphans(&t, Vec::new(), true);
    assert!(at(&fs, ".config/GutCheck").is_some());
}

#[test]
fn uq_family_state_is_claimed() {
    let t = FixtureTree::new();
    for d in [
        ".local/share/uq/themes",
        ".local/share/uq-dev/pkg",
        ".local/share/notifd.prev",
        ".local/state/notifd",
        ".local/state/notify/backups",
    ] {
        t.mkdir(&format!("/home/user/{d}"));
    }
    let fs = orphans(&t, vec![pacman("uq", &[]), pacman("noti-fi", &[])], true);
    assert!(fs.is_empty(), "{fs:#?}");
}
