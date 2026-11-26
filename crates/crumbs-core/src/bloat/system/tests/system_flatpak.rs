use super::*;

#[test]
fn system_temp_item() {
    let t = FixtureTree::new();
    t.write_exec("usr/bin/systemd-tmpfiles", b"#!/bin/sh\n");
    let runner = FakeRunner::new();
    let rep = scan(&t, &runner, Category::SystemTemp);
    assert_eq!(rep.items.len(), 1);
    let i = &rep.items[0];
    assert_eq!(i.size, None);
    assert_eq!(
        i.action,
        priv_action(
            "systemd-tmpfiles",
            &["--clean", "--prefix=/tmp", "--prefix=/var/tmp"]
        )
    );
    assert_eq!(i.scope, Scope::System);
    assert!(i.preselected);
    assert!(i.note.as_deref().unwrap().contains("/tmp and /var/tmp"));
    assert!(runner.calls().is_empty());
}

#[test]
fn flatpak_unused_system_opt_in() {
    let t = FixtureTree::new();
    t.write_exec("usr/bin/flatpak", b"#!/bin/sh\n");
    let runner = FakeRunner::new().on(
        "flatpak",
        &FLATPAK_LIST,
        out(0, "org.freedesktop.Platform/x86_64/23.08\n", ""),
    );
    let rep = scan(&t, &runner, Category::FlatpakUnusedSystem);
    assert_eq!(rep.items.len(), 1);
    let i = &rep.items[0];
    assert!(!i.preselected, "flatpak-unused-system is opt-in");
    assert_eq!(i.size, None);
    assert_eq!(
        i.action,
        priv_action(
            "flatpak",
            &[
                "uninstall",
                "--system",
                "--unused",
                "-y",
                "--noninteractive"
            ]
        )
    );
    assert_eq!(i.scope, Scope::System);

    let runner = FakeRunner::new().on("flatpak", &FLATPAK_LIST, out(0, "\n", ""));
    assert!(
        scan(&t, &runner, Category::FlatpakUnusedSystem)
            .items
            .is_empty()
    );
    let runner = FakeRunner::new().on("flatpak", &FLATPAK_LIST, out(1, "", "boom\n"));
    let rep = scan(&t, &runner, Category::FlatpakUnusedSystem);
    assert!(rep.error.unwrap().contains("boom"));
}

#[test]
fn flatpak_unused_system_follows_flatpak() {
    let t = FixtureTree::new();
    t.write_exec("usr/bin/flatpak", b"#!/bin/sh\n");
    let probe = ["uninstall", "--system", "--unused"];
    let list = || out(0, "org.gnome.Platform/x86_64/45\n", "");
    let version = || out(0, "Flatpak 1.18.4\n", "");
    let none = FakeRunner::new()
        .on("flatpak", &FLATPAK_LIST, list())
        .on("flatpak", &["--version"], version())
        .on(
            "flatpak",
            &probe,
            out(0, "Nothing unused to uninstall\n", ""),
        );
    assert!(
        scan(&t, &none, Category::FlatpakUnusedSystem)
            .items
            .is_empty()
    );
    let some = FakeRunner::new()
        .on("flatpak", &FLATPAK_LIST, list())
        .on("flatpak", &["--version"], version())
        .on(
            "flatpak",
            &probe,
            out(
                1,
                " 1.     org.gnome.Platform   45   r\n",
                "error: Aborted by user\n",
            ),
        );
    let items = scan(&t, &some, Category::FlatpakUnusedSystem).items;
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].label, "1 unused Flatpak runtime (system)");
    assert_eq!(items[0].note.as_deref(), Some("org.gnome.Platform//45"));
    assert!(!items[0].preselected);
}

#[test]
fn unavailable_category_note() {
    let t = FixtureTree::new();
    let runner = FakeRunner::new();
    let cases = [
        (Category::PkgCache, "pacman, apt-get or dnf not installed"),
        (Category::OrphanPkgs, "pacman, apt-get or dnf not installed"),
        (Category::Journal, "journalctl not installed"),
        (Category::SystemTemp, "systemd-tmpfiles not installed"),
        (Category::FlatpakUnusedSystem, "flatpak not installed"),
        (Category::Coredumps, "no coredump directory"),
    ];
    for (cat, reason) in cases {
        let rep = scan(&t, &runner, cat);
        assert_eq!(rep.unavailable.as_deref(), Some(reason), "{cat:?}");
        assert!(rep.items.is_empty() && rep.error.is_none(), "{cat:?}");
        let v = serde_json::to_value(&rep).unwrap();
        assert_eq!(v["unavailable"], reason);
    }
    assert!(runner.calls().is_empty());

    // Available categories omit the field.
    let t = FixtureTree::new();
    t.write_exec("usr/bin/systemd-tmpfiles", b"#!/bin/sh\n");
    let v = serde_json::to_value(scan(&t, &runner, Category::SystemTemp)).unwrap();
    assert!(v.get("unavailable").is_none());
}

#[test]
fn registry_follows_category_order() {
    let cats: Vec<Category> = scanners().iter().map(|s| s.category()).collect();
    assert_eq!(cats, Category::ALL);
}

#[test]
fn apt_pkg_cache_size_from_archives() {
    let t = FixtureTree::new();
    t.write_exec("usr/bin/apt-get", b"#!/bin/sh\n");
    t.write("var/cache/apt/archives/a.deb", &[0u8; 1000]);
    t.write("var/cache/apt/archives/b.deb", &[0u8; 2000]);
    t.write("var/cache/apt/archives/partial/c.deb", &[0u8; 4000]);
    t.write("var/cache/apt/archives/lock", b"");
    let runner = FakeRunner::new();
    let rep = scan(&t, &runner, Category::PkgCache);
    assert!(rep.error.is_none() && rep.unavailable.is_none(), "{rep:?}");
    assert_eq!(rep.items.len(), 1);
    assert_eq!(rep.items[0].size, Some(3000));
    assert_eq!(rep.items[0].action, priv_action("apt-get", &["clean"]));
    assert!(rep.items[0].preselected);

    // Nothing cached: no item.
    let t = FixtureTree::new();
    t.write_exec("usr/bin/apt-get", b"#!/bin/sh\n");
    assert!(scan(&t, &runner, Category::PkgCache).items.is_empty());
}

#[test]
fn apt_orphans_exact_purge_list() {
    let t = FixtureTree::new();
    t.write_exec("usr/bin/apt-get", b"#!/bin/sh\n");
    t.write(
        "var/lib/dpkg/status",
        b"Package: cowsay\nStatus: install ok installed\nInstalled-Size: 100\n\nPackage: netbase\nStatus: install ok installed\nInstalled-Size: 50\n\n",
    );
    let runner = FakeRunner::new().on(
        "apt-get",
        &["-s", "autoremove", "--purge"],
        out(0, APT_AUTOREMOVE, ""),
    );
    let rep = scan(&t, &runner, Category::OrphanPkgs);
    assert_eq!(rep.items.len(), 1, "{rep:?}");
    let i = &rep.items[0];
    let ItemAction::Privileged { program, args } = &i.action else {
        panic!("{i:?}");
    };
    assert_eq!(program, "dpkg");
    assert_eq!(args[0], "--purge");
    assert_eq!(args.len(), 1 + 8);
    assert!(args.contains(&"netbase".to_string()));
    assert!(!args.iter().any(|a| a.contains("autoremove")));
    assert_eq!(i.size, Some(150 * 1024));
    assert!(!i.preselected);

    // Nothing to remove: no item.
    let runner = FakeRunner::new().on(
        "apt-get",
        &["-s", "autoremove", "--purge"],
        out(
            0,
            "0 upgraded, 0 newly installed, 0 to remove and 0 not upgraded.\n",
            "",
        ),
    );
    assert!(scan(&t, &runner, Category::OrphanPkgs).items.is_empty());
}

#[test]
fn dnf5_pkg_cache_size() {
    let t = FixtureTree::new();
    t.write_exec("usr/bin/dnf", b"#!/bin/sh\n");
    t.write("var/cache/libdnf5/fedora-1a2b/packages/a.rpm", &[0u8; 500]);
    t.write("var/cache/libdnf5/updates-3c4d/packages/b.rpm", &[0u8; 700]);
    t.write(
        "var/cache/libdnf5/fedora-1a2b/repodata/primary.xml.zst",
        &[0u8; 9000],
    );
    let rep = scan(&t, &FakeRunner::new(), Category::PkgCache);
    assert_eq!(rep.items.len(), 1, "{rep:?}");
    assert_eq!(rep.items[0].size, Some(1200));
    assert_eq!(
        rep.items[0].action,
        priv_action("dnf", &["clean", "packages"])
    );
}

#[test]
fn dnf_orphans_exact_remove_list() {
    let t = FixtureTree::new();
    t.write_exec("usr/bin/dnf", b"#!/bin/sh\n");
    let runner = FakeRunner::new().on(
        "dnf",
        &["autoremove", "--assumeno"],
        out(1, DNF5_AUTOREMOVE, ""),
    );
    let rep = scan(&t, &runner, Category::OrphanPkgs);
    assert_eq!(rep.items.len(), 1, "{rep:?}");
    assert_eq!(
        rep.items[0].action,
        priv_action("rpm", &["-e", "cowsay", "perl-Text-CharWidth"])
    );
    assert!(!rep.items[0].preselected);

    let runner = FakeRunner::new().on(
        "dnf",
        &["autoremove", "--assumeno"],
        out(0, "Nothing to do.\n", ""),
    );
    assert!(scan(&t, &runner, Category::OrphanPkgs).items.is_empty());
}

#[test]
fn snapshots_offers_timeshift_pruning() {
    let t = FixtureTree::new();
    t.write_exec("usr/bin/timeshift", b"#!/bin/sh\n");
    t.write_exec("usr/bin/sh", b"#!/bin/sh\n");
    t.write(
        "etc/timeshift/timeshift.json",
        b"{\"btrfs_mode\" : \"true\"}",
    );
    let rep = scan(&t, &FakeRunner::new(), Category::Snapshots);
    assert!(rep.error.is_none() && rep.unavailable.is_none(), "{rep:?}");
    assert_eq!(rep.items.len(), 1);
    let i = &rep.items[0];
    assert_eq!(
        i.label,
        "Timeshift snapshots beyond the newest 3, oldest first (counted when cleaned)"
    );
    assert_eq!(i.size, None);
    assert!(!i.preselected, "opt-in");
    assert_eq!(i.scope, Scope::System);
    assert_eq!(
        i.action,
        ItemAction::Privileged {
            program: "sh".into(),
            args: vec![
                "-c".into(),
                super::super::snapshots::PRUNE_TIMESHIFT.into(),
                "crumbs-snapshots".into(),
                "3".into()
            ],
        }
    );
    // The keep count comes from the config.
    let cfg = Config {
        snapshots_keep: 5,
        ..Config::default()
    };
    let rep = scan_cfg(&t, &FakeRunner::new(), &cfg, Category::Snapshots);
    assert!(
        rep.items[0]
            .label
            .starts_with("Timeshift snapshots beyond the newest 5,")
    );
}

#[test]
fn snapshots_unavailable_without_timeshift_setup() {
    let t = FixtureTree::new();
    let rep = scan(&t, &FakeRunner::new(), Category::Snapshots);
    assert_eq!(rep.unavailable.as_deref(), Some("timeshift not installed"));
    // Installed but never set up: nothing to prune.
    t.write_exec("usr/bin/timeshift", b"#!/bin/sh\n");
    let rep = scan(&t, &FakeRunner::new(), Category::Snapshots);
    assert!(rep.unavailable.is_none() && rep.items.is_empty(), "{rep:?}");
}

#[test]
fn prune_script_deletes_all_but_the_newest() {
    let (ok, deleted) = run_prune(LISTING, "2");
    assert!(ok);
    assert_eq!(deleted, ["2026-10-24_12-00-00", "2026-11-08_14-00-06"]);
    let (ok, deleted) = run_prune(LISTING, "4");
    assert!(ok && deleted.is_empty(), "{deleted:?}");
    // Nothing listed: nothing deleted.
    let (ok, deleted) = run_prune("No snapshots found\n", "1");
    assert!(ok && deleted.is_empty());
}

#[test]
fn prune_script_refuses_unexpected_names_and_counts() {
    let odd =
        "0    >  2026-11-08_14-00-06  D\n1    >  ../../etc  O\n2    >  2026-11-10_08-18-24  O\n";
    let (ok, deleted) = run_prune(odd, "1");
    assert!(!ok, "an unexpected name stops the step");
    assert!(deleted.is_empty(), "{deleted:?}");
    for bad in ["0", "-1", "x", ""] {
        let (ok, deleted) = run_prune(LISTING, bad);
        assert!(!ok && deleted.is_empty(), "keep {bad:?}");
    }
}
