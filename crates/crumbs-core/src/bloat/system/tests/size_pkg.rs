use super::*;

#[test]
fn size_parsing() {
    assert_eq!(parse_size("86.17 MiB"), Some(90_355_794));
    assert_eq!(parse_size("719.1M"), Some(754_031_002));
    assert_eq!(parse_size("8.0K"), Some(8192));
    assert_eq!(parse_size("1G"), Some(1 << 30));
    assert_eq!(parse_size("12 B"), Some(12));
    assert_eq!(parse_size("1 furlongs"), None);
    assert_eq!(scale_1024(2.0, "TiB"), Some(2 << 40));
    assert_eq!(journal_size_bytes("500M"), Some(500 << 20));
    assert_eq!(journal_size_bytes("1024"), Some(1024));
    assert_eq!(journal_size_bytes("2T"), Some(2 << 40));
    for bad in ["", "M", "1.5G", "-1M", "5 M", "5m", "99999999999999999999T"] {
        assert_eq!(journal_size_bytes(bad), None, "{bad}");
    }
}

#[test]
fn pkg_cache_parses_dry_run() {
    let t = FixtureTree::new();
    t.write_exec("usr/bin/pacman", b"#!/bin/sh\n");
    t.write_exec("usr/bin/paccache", b"#!/bin/sh\n");
    let runner = FakeRunner::new().on("paccache", &["-dk2"], out(0, PACCACHE_DRY, ""));
    let rep = scan(&t, &runner, Category::PkgCache);
    assert!(rep.error.is_none() && rep.unavailable.is_none(), "{rep:?}");
    assert_eq!(rep.items.len(), 1);
    let i = &rep.items[0];
    assert_eq!(i.category, Category::PkgCache);
    assert_eq!(i.size, Some(90_355_794));
    assert_eq!(i.action, priv_action("paccache", &["-rk2"]));
    assert_eq!(i.scope, Scope::System);
    assert!(i.path.is_none() && i.preselected && i.fingerprint.is_none());
    assert_eq!(runner.calls(), ["paccache -dk2", "paccache -duk0"]);

    // The configured keep count is used.
    let cfg = Config {
        pkg_cache_keep: 0,
        ..Config::default()
    };
    let runner = FakeRunner::new().on("paccache", &["-dk0"], out(0, PACCACHE_DRY, ""));
    let rep = scan_cfg(&t, &runner, &cfg, Category::PkgCache);
    assert_eq!(rep.items[0].action, priv_action("paccache", &["-rk0"]));

    // Nothing to prune: no item.
    let runner = FakeRunner::new().on(
        "paccache",
        &["-dk2"],
        out(0, "==> no candidate packages found for pruning\n", ""),
    );
    let rep = scan(&t, &runner, Category::PkgCache);
    assert!(rep.items.is_empty() && rep.error.is_none() && rep.unavailable.is_none());

    // A failing paccache is a scanner error.
    let runner = FakeRunner::new().on("paccache", &["-dk2"], out(1, "", "boom\n"));
    let rep = scan(&t, &runner, Category::PkgCache);
    assert!(rep.error.as_deref().unwrap().contains("boom"));
}

#[test]
fn pkg_cache_offers_packages_no_longer_installed() {
    let t = FixtureTree::new();
    t.write_exec("usr/bin/pacman", b"#!/bin/sh\n");
    t.write_exec("usr/bin/paccache", b"#!/bin/sh\n");
    let none = "==> no candidate packages found for pruning\n";
    let runner = FakeRunner::new()
        .on("paccache", &["-dk2"], out(0, none, ""))
        .on("paccache", &["-duk0"], out(0, PACCACHE_DRY, ""));
    let rep = scan(&t, &runner, Category::PkgCache);
    assert_eq!(rep.items.len(), 1, "{rep:?}");
    let i = &rep.items[0];
    assert_eq!(i.label, "cached packages no longer installed");
    assert_eq!(i.size, Some(90_355_794));
    assert_eq!(i.action, priv_action("paccache", &["-ruk0"]));
    assert!(i.preselected && i.rebuildable);
    // Both: two items, the uninstalled ones first.
    let runner = FakeRunner::new()
        .on("paccache", &["-dk2"], out(0, PACCACHE_DRY, ""))
        .on("paccache", &["-duk0"], out(0, PACCACHE_DRY, ""));
    let rep = scan(&t, &runner, Category::PkgCache);
    assert_eq!(rep.items.len(), 2);
    // A failing uninstalled dry run only drops that item.
    let runner = FakeRunner::new()
        .on("paccache", &["-dk2"], out(0, PACCACHE_DRY, ""))
        .on("paccache", &["-duk0"], out(1, "", "boom\n"));
    let rep = scan(&t, &runner, Category::PkgCache);
    assert!(rep.error.is_none(), "{rep:?}");
    assert_eq!(rep.items.len(), 1);
    assert_eq!(rep.items[0].action, priv_action("paccache", &["-rk2"]));
}

#[test]
fn pkg_cache_without_paccache() {
    let t = FixtureTree::new();
    t.write_exec("usr/bin/pacman", b"#!/bin/sh\n");
    let runner = FakeRunner::new();
    let rep = scan(&t, &runner, Category::PkgCache);
    assert_eq!(rep.items.len(), 1);
    let i = &rep.items[0];
    assert_eq!(i.size, None);
    assert_eq!(i.action, priv_action("pacman", &["-Sc", "--noconfirm"]));
    assert_eq!(
        i.note.as_deref(),
        Some(
            "keeps only installed versions and also removes unused repository sync databases (pacman-contrib not installed)"
        )
    );
    assert!(runner.calls().is_empty());
}

#[test]
fn orphan_pkgs_item_and_opt_in() {
    let (t, runner) = orphan_fixture();
    let runner = runner.on(
        "pacman",
        &SIM,
        out(0, "foo 1.0-1\nbar 2.0-1\nlibbaz 3-1\n", ""),
    );
    let rep = scan(&t, &runner, Category::OrphanPkgs);
    assert!(rep.error.is_none(), "{rep:?}");
    assert_eq!(rep.items.len(), 1);
    let i = &rep.items[0];
    assert_eq!(i.label, "3 orphaned packages");
    assert_eq!(i.size, Some(6000));
    assert!(!i.preselected, "orphan-pkgs is opt-in");
    assert_eq!(i.note.as_deref(), Some("foo, bar, libbaz"));
    // The exact simulated closure, no -s.
    assert_eq!(
        i.action,
        priv_action("pacman", &["-Rn", "--noconfirm", "foo", "bar", "libbaz"])
    );
    assert_eq!(i.scope, Scope::System);
    assert_eq!(rep.total, 6000);
}

#[test]
fn orphan_pkgs_note_truncates_after_five() {
    let t = FixtureTree::new();
    t.write_exec("usr/bin/pacman", b"#!/bin/sh\n");
    let names = ["a", "b", "c", "d", "e", "f", "g"];
    for n in names {
        t.pacman_pkg(n, "1-1", Some(10), &[]);
    }
    let mut sim = vec!["-Rs", "--print", "--print-format", "%n %v"];
    sim.extend(names);
    let listing: String = names.iter().map(|n| format!("{n}\n")).collect();
    let versions: String = names.iter().map(|n| format!("{n} 1-1\n")).collect();
    let runner = FakeRunner::new()
        .on("pacman", &["-Qdtq"], out(0, &listing, ""))
        .on("pacman", &sim, out(0, &versions, ""));
    let rep = scan(&t, &runner, Category::OrphanPkgs);
    let i = &rep.items[0];
    assert_eq!(i.label, "7 orphaned packages");
    assert_eq!(i.note.as_deref(), Some("a, b, c, d, e, …"));
    assert_eq!(i.size, Some(70));
}

#[test]
fn orphan_pkgs_none_when_pacman_has_no_orphans() {
    let t = FixtureTree::new();
    t.write_exec("usr/bin/pacman", b"#!/bin/sh\n");
    // pacman -Qdtq exits 1 with no output when there are no targets.
    let runner = FakeRunner::new().on("pacman", &["-Qdtq"], out(1, "", ""));
    let rep = scan(&t, &runner, Category::OrphanPkgs);
    assert!(rep.items.is_empty() && rep.error.is_none());
}

#[test]
fn orphan_pkgs_simulation_error() {
    let (t, runner) = orphan_fixture();
    let runner = runner.on(
        "pacman",
        &SIM,
        out(
            1,
            ":: removing foo breaks dependency 'foo' required by keeper\n",
            "error: failed to prepare transaction (could not satisfy dependencies)\n",
        ),
    );
    let rep = scan(&t, &runner, Category::OrphanPkgs);
    assert!(rep.items.is_empty());
    let e = rep.error.unwrap();
    assert!(
        e.contains("breaks dependency 'foo' required by keeper"),
        "{e}"
    );
    assert!(e.contains("failed to prepare transaction"), "{e}");
}

#[test]
fn orphan_pkgs_refuses_odd_names() {
    let t = FixtureTree::new();
    t.write_exec("usr/bin/pacman", b"#!/bin/sh\n");
    let runner = FakeRunner::new().on("pacman", &["-Qdtq"], out(0, "-rf\n", ""));
    let rep = scan(&t, &runner, Category::OrphanPkgs);
    assert!(rep.items.is_empty());
    assert!(rep.error.is_some());
}

#[test]
fn journal_over_limit() {
    let t = FixtureTree::new();
    t.write_exec("usr/bin/journalctl", b"#!/bin/sh\n");
    let runner = FakeRunner::new().on("journalctl", &["--disk-usage"], out(0, JOURNAL, ""));
    let cfg = Config {
        journal_max: "700M".into(),
        ..Config::default()
    };
    let rep = scan_cfg(&t, &runner, &cfg, Category::Journal);
    assert!(rep.error.is_none(), "{rep:?}");
    assert_eq!(rep.items.len(), 1);
    let i = &rep.items[0];
    assert_eq!(i.size, Some(754_031_002 - 700 * 1024 * 1024));
    assert_eq!(i.action, priv_action("journalctl", &["--vacuum-size=700M"]));
    assert_eq!(i.scope, Scope::System);
    assert!(i.preselected);
}

#[test]
fn journal_under_limit_no_item() {
    let t = FixtureTree::new();
    t.write_exec("usr/bin/journalctl", b"#!/bin/sh\n");
    let runner = FakeRunner::new().on("journalctl", &["--disk-usage"], out(0, JOURNAL, ""));
    // The default limit (500M) is below 719.1M; raise it.
    let cfg = Config {
        journal_max: "1G".into(),
        ..Config::default()
    };
    let rep = scan_cfg(&t, &runner, &cfg, Category::Journal);
    assert!(rep.items.is_empty() && rep.error.is_none() && rep.unavailable.is_none());
}

#[test]
fn journal_max_fails_closed_and_boundaries() {
    let t = FixtureTree::new();
    t.write_exec("usr/bin/journalctl", b"#!/bin/sh\n");
    let mk = || FakeRunner::new().on("journalctl", &["--disk-usage"], out(0, JOURNAL, ""));
    for bad in ["1;x", "-1M", "0", "0M", "", "$(id)"] {
        let cfg = Config {
            journal_max: bad.into(),
            ..Config::default()
        };
        let rep = scan_cfg(&t, &mk(), &cfg, Category::Journal);
        assert!(rep.items.is_empty(), "{bad:?}");
        assert!(rep.error.is_some(), "{bad:?}");
    }
    // usage == max: nothing to vacuum.
    let usage = out(
        0,
        "Archived and active journals take up 1M in the file system.\n",
        "",
    );
    let runner = FakeRunner::new().on("journalctl", &["--disk-usage"], usage);
    let cfg = Config {
        journal_max: "1M".into(),
        ..Config::default()
    };
    let rep = scan_cfg(&t, &runner, &cfg, Category::Journal);
    assert!(rep.items.is_empty() && rep.error.is_none());
    // A 1T limit works through the scanner (no item: usage is below it).
    let cfg = Config {
        journal_max: "1T".into(),
        ..Config::default()
    };
    let rep = scan_cfg(&t, &mk(), &cfg, Category::Journal);
    assert!(rep.items.is_empty() && rep.error.is_none());
    let big = out(
        0,
        "Archived and active journals take up 2T in the file system.\n",
        "",
    );
    let runner = FakeRunner::new().on("journalctl", &["--disk-usage"], big);
    let rep = scan_cfg(&t, &runner, &cfg, Category::Journal);
    assert_eq!(rep.items[0].size, Some(1 << 40));
    assert_eq!(
        rep.items[0].action,
        priv_action("journalctl", &["--vacuum-size=1T"])
    );
}

#[test]
fn coredumps_items_system_scope() {
    let t = FixtureTree::new();
    let dir = "/var/lib/systemd/coredump";
    t.write(&format!("{dir}/core.foo.1000.abc.123.zst"), &[7u8; 9000]);
    t.write(&format!("{dir}/core.bar.1000.abc.456.zst"), &[7u8; 100]);
    t.write(&format!("{dir}/notacore.txt"), b"x");
    t.write(&format!("{dir}/core."), b"x");
    t.symlink(&format!("{dir}/core.link"), "/etc/passwd");
    t.mkdir(&format!("{dir}/core.dir"));
    let runner = FakeRunner::new();
    let rep = scan(&t, &runner, Category::Coredumps);
    assert!(rep.error.is_none() && rep.unavailable.is_none(), "{rep:?}");
    let labels: Vec<&str> = rep.items.iter().map(|i| i.label.as_str()).collect();
    assert_eq!(
        labels,
        ["core.foo.1000.abc.123.zst", "core.bar.1000.abc.456.zst"]
    );
    let env = t.env();
    for i in &rep.items {
        assert_eq!(i.scope, Scope::System);
        assert_eq!(i.action, ItemAction::Delete);
        assert!(i.fingerprint.is_some() && i.preselected);
        let p = i.path.clone().unwrap();
        assert!(p.starts_with(dir), "in-system path: {p:?}");
        let blocks = std::fs::symlink_metadata(env.sys(p.to_str().unwrap()))
            .unwrap()
            .blocks()
            * 512;
        assert_eq!(i.size, Some(blocks));
    }
    assert!(runner.calls().is_empty());
}
