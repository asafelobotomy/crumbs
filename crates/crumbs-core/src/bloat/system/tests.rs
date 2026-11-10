#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::os::unix::fs::MetadataExt;
use std::time::SystemTime;

use super::super::*;
use super::{parse_size, scale_1024};
use crate::command::CmdOutput;
use crate::config::journal_size_bytes;
use crate::testing::{FakeRunner, FixtureTree};

const PACCACHE_DRY: &str = "==> finished dry run: 1 candidates (disk space saved: 86.17 MiB)\n";
const JOURNAL: &str = "Archived and active journals take up 719.1M in the file system.\n";
const FLATPAK_LIST: [&str; 4] = ["list", "--system", "--runtime", "--columns=ref"];

fn out(status: i32, stdout: &str, stderr: &str) -> CmdOutput {
    CmdOutput {
        status,
        stdout: stdout.into(),
        stderr: stderr.into(),
        timed_out: false,
    }
}

fn strings(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

fn scan_cfg(t: &FixtureTree, runner: &FakeRunner, cfg: &Config, cat: Category) -> CategoryReport {
    let env = t.env();
    let inv = Inventory {
        apps: vec![],
        statuses: vec![],
        errors: vec![],
        owned: Default::default(),
    };
    let cx = ScanContext {
        env: &env,
        cfg,
        inv: &inv,
        runner,
        now: SystemTime::now(),
    };
    scan_all(&cx, &[cat]).categories.remove(0)
}

fn scan(t: &FixtureTree, runner: &FakeRunner, cat: Category) -> CategoryReport {
    scan_cfg(t, runner, &Config::default(), cat)
}

fn priv_action(program: &str, args: &[&str]) -> ItemAction {
    ItemAction::Privileged {
        program: program.into(),
        args: strings(args),
    }
}

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

fn orphan_fixture() -> (FixtureTree, FakeRunner) {
    let t = FixtureTree::new();
    t.write_exec("usr/bin/pacman", b"#!/bin/sh\n");
    t.pacman_pkg("foo", "1.0-1", Some(1000), &[]);
    t.pacman_pkg("bar", "2.0-1", Some(2000), &[]);
    t.pacman_pkg("libbaz", "3-1", Some(3000), &[]);
    t.pacman_pkg("keeper", "9-1", Some(9_000_000), &[]);
    let runner = FakeRunner::new().on("pacman", &["-Qdtq"], out(0, "foo\nbar\n", ""));
    (t, runner)
}

const SIM: [&str; 6] = ["-Rs", "--print", "--print-format", "%n %v", "foo", "bar"];

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

const APT_AUTOREMOVE: &str = include_str!("../../../tests/fixtures/apt/autoremove.txt");
const DNF5_AUTOREMOVE: &str = include_str!("../../../tests/fixtures/dnf/dnf5-autoremove.txt");

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
