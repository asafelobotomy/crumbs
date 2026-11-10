use super::*;
use crate::testing::FixtureTree;

fn declared(lines: &str) -> TmpfilesDeclared {
    let mut d = TmpfilesDeclared::default();
    for l in lines.lines() {
        d.add_line(l);
    }
    d
}

#[test]
fn detect_prefers_pacman_then_dpkg_then_rpm() {
    let t = FixtureTree::new();
    assert_eq!(detect_package_db(&t.env()), None);
    t.write_exec("/usr/bin/rpm", b"#!/bin/sh\n");
    assert_eq!(detect_package_db(&t.env()), None);
    t.write("/usr/lib/sysimage/rpm/rpmdb.sqlite", b"x");
    assert_eq!(detect_package_db(&t.env()), Some(PackageDb::Rpm));
    t.write("/var/lib/dpkg/status", b"");
    assert_eq!(detect_package_db(&t.env()), Some(PackageDb::Rpm));
    t.write("/var/lib/dpkg/status", b"Package: x\n");
    t.mkdir("/var/lib/dpkg/info");
    assert_eq!(detect_package_db(&t.env()), Some(PackageDb::Dpkg));
    t.write("/var/lib/pacman/local/ALPM_DB_VERSION", b"9\n");
    assert_eq!(detect_package_db(&t.env()), Some(PackageDb::Pacman));
}

#[test]
fn load_package_db_combines_every_present_database() {
    let t = FixtureTree::new();
    t.write(
        "/var/lib/pacman/local/foo-1-1/files",
        b"%FILES%\nopt/foo/\nopt/foo/bin\n\n",
    );
    t.write(
        "/var/lib/dpkg/status",
        b"Package: bar\nStatus: install ok installed\n\n",
    );
    t.write(
        "/var/lib/dpkg/info/bar.list",
        b"/.\n/opt/bar\n/opt/bar/run\n",
    );
    let runner = crate::testing::FakeRunner::new();
    let idx = load_package_db(&t.env(), &runner).unwrap().unwrap();
    assert!(idx.owns_at_or_below(Path::new("/opt/foo")));
    assert!(idx.owns_at_or_below(Path::new("/opt/bar")));
    // An unreadable present database fails the whole index.
    std::fs::remove_dir_all(t.env().sys("/var/lib/dpkg/info")).unwrap();
    std::fs::create_dir(t.env().sys("/var/lib/dpkg/info")).unwrap();
    t.write("/var/lib/dpkg/info/broken.list", b"");
    std::fs::remove_file(t.env().sys("/var/lib/dpkg/info/broken.list")).unwrap();
    std::fs::create_dir(t.env().sys("/var/lib/dpkg/info/broken.list")).unwrap();
    assert!(load_package_db(&t.env(), &runner).is_err());
}

#[test]
fn load_package_db_dispatches_to_pacman() {
    let t = FixtureTree::new();
    t.write(
        "/var/lib/pacman/local/foo-1-1/files",
        b"%FILES%\nopt/foo/\nopt/foo/bin\n\n",
    );
    let runner = crate::testing::FakeRunner::new();
    let idx = load_package_db(&t.env(), &runner).unwrap().unwrap();
    assert!(idx.owns_at_or_below(Path::new("/opt/foo")));
    assert_eq!(
        load_package_db(&FixtureTree::new().env(), &runner).unwrap(),
        None
    );
}

fn covers(d: &TmpfilesDeclared, p: &str) -> bool {
    d.covers(Path::new(p))
}

#[test]
fn tmpfiles_paths_are_parsed() {
    let t = FixtureTree::new();
    t.write(
        "usr/lib/tmpfiles.d/a.conf",
        b"# c\n\nd /etc/openvpn/client 0750 o n -\nd /run/%u 0700\nz /opt/x - - -\n",
    );
    t.write("etc/tmpfiles.d/b.conf", b"f /etc/y 0644\n");
    t.write("etc/tmpfiles.d/b.txt", b"f /etc/ignored 0644\n");
    t.write("usr/local/lib/tmpfiles.d/c.conf", b"d /opt/local-dir\n");
    t.write("run/tmpfiles.d/d.conf", b"d /etc/runtime-dir\nd %S/state\n");
    let got = tmpfiles_declared(&t.env()).unwrap();
    for p in [
        "/etc/openvpn",
        "/opt/x",
        "/etc/y",
        "/opt/local-dir",
        "/etc/runtime-dir",
    ] {
        assert!(covers(&got, p), "{p}");
    }
    for p in ["/etc/ignored", "/opt/xy", "/etc/state", "/opt/state"] {
        assert!(!covers(&got, p), "{p}");
    }
    assert_eq!(
        tmpfiles_declared(&FixtureTree::new().env()).unwrap(),
        TmpfilesDeclared::default()
    );
}

#[test]
fn tmpfiles_patterns_cover_only_what_they_can_match() {
    // systemd-nspawn.conf: a pattern in `/` is shallower than any
    // child of /opt.
    let d = declared("R! /.#machine.*\n");
    assert!(!covers(&d, "/opt/inky"));
    assert!(!covers(&d, "/etc/inky"));
    let d = declared("C /etc/tpm2-tss/fapi-profiles/*.json - - - -\n");
    assert!(covers(&d, "/etc/tpm2-tss"));
    assert!(!covers(&d, "/etc/tpm2"));
    let d = declared("d /opt/foo*\n");
    assert!(covers(&d, "/opt/foobar"));
    assert!(covers(&d, "/opt/foo"));
    assert!(!covers(&d, "/opt/bar"));
    let d = declared("x /op*\n");
    assert!(!covers(&d, "/opt/inky"));
    // A declared ancestor covers nothing below it.
    let d = declared("z /\nz /opt\n");
    assert!(!covers(&d, "/opt/inky"));
    assert!(!covers(&d, "/etc/inky"));
    // Specifiers match like `*`, within one component.
    let d = declared("d /opt/inky%b/run\nd /etc/%m.d/x\n");
    assert!(covers(&d, "/opt/inky-1"));
    assert!(!covers(&d, "/opt/other"));
    assert!(covers(&d, "/etc/abc.d"));
    assert!(!covers(&d, "/etc/abc"));
    // Classes, negation and ranges.
    let d = declared("d /opt/tpm[0-9]/x\nd /etc/[!a]x\nd /etc/[^b]y\n");
    assert!(covers(&d, "/opt/tpm3"));
    assert!(!covers(&d, "/opt/tpmx"));
    assert!(covers(&d, "/etc/bx") && !covers(&d, "/etc/ax"));
    assert!(covers(&d, "/etc/ay") && !covers(&d, "/etc/by"));
    let d = declared("d /opt/a?c\n");
    assert!(covers(&d, "/opt/abc") && !covers(&d, "/opt/ac"));
}

#[test]
fn tmpfiles_specifier_e_and_quoting() {
    let d = declared("d %E/inky 0755\n");
    assert!(covers(&d, "/etc/inky"));
    let d = declared("d \"/opt/foo bar\" 0755\n");
    assert!(covers(&d, "/opt/foo bar"));
    assert!(!covers(&d, "/opt/foo"));
    let d = declared("d /opt/a\\x20b 0755\n");
    assert!(covers(&d, "/opt/a b"));
    // Cannot be unquoted: covers everything its literal directory may
    // hold.
    let d = declared("d \"/opt/broken 0755\n");
    assert!(covers(&d, "/opt/inky"));
    assert!(!covers(&d, "/etc/inky"));
    let d = declared("d /etc/x/y\\q 0755\n");
    assert!(covers(&d, "/etc/x"));
    assert!(!covers(&d, "/etc/inky"));
}

#[test]
fn dirs_from_files_section_are_owned() {
    let t = FixtureTree::new();
    t.pacman_pkg("foo", "1-1", Some(1), &["opt/foo/", "opt/foo/bin/foo"]);
    let idx = load_pacman(&t.env()).unwrap().unwrap();
    assert!(idx.owns(Path::new("/opt/foo")));
    assert!(idx.owns_at_or_below(Path::new("/opt/foo")));
    assert!(idx.owns(Path::new("/opt/foo/bin/foo")));
    assert!(!idx.is_empty());
}

#[test]
fn prefix_is_component_wise() {
    let t = FixtureTree::new();
    t.pacman_pkg("foobar", "1-1", Some(1), &["opt/foobar/x"]);
    let idx = load_pacman(&t.env()).unwrap().unwrap();
    assert!(!idx.owns_at_or_below(Path::new("/opt/foo")));
    assert!(idx.owns_at_or_below(Path::new("/opt/foobar")));
}

#[test]
fn unowned_tree_is_unowned() {
    let t = FixtureTree::new();
    t.pacman_pkg("foo", "1-1", Some(1), &["opt/foo/"]);
    let idx = load_pacman(&t.env()).unwrap().unwrap();
    assert!(!idx.owns(Path::new("/opt/inky")));
    assert!(!idx.owns_at_or_below(Path::new("/opt/inky")));
}

#[test]
fn no_pacman_db_is_none() {
    let t = FixtureTree::new();
    assert_eq!(load_pacman(&t.env()).unwrap(), None);
}

#[test]
fn non_utf8_owned_dir_is_owned() {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;
    let t = FixtureTree::new();
    t.write(
        "/var/lib/pacman/local/caf-1-1/files",
        b"%FILES%\nopt/caf\xe9/\nopt/caf\xe9/x\n",
    );
    let idx = load_pacman(&t.env()).unwrap().unwrap();
    let dir = Path::new(OsStr::from_bytes(b"/opt/caf\xe9"));
    assert!(idx.owns(dir));
    assert!(idx.owns_dir(dir));
    assert!(idx.owns_at_or_below(dir));
    assert!(!idx.owns_at_or_below(Path::new("/opt/caf\u{fffd}")));
}

#[test]
fn unreadable_files_entry_is_an_error() {
    let t = FixtureTree::new();
    t.pacman_pkg("ok", "1-1", Some(1), &["opt/ok/"]);
    // `files` is a directory, so reading it fails.
    t.write("/var/lib/pacman/local/bad-1-1/files/x", b"");
    assert!(load_pacman(&t.env()).is_err());
}

#[test]
fn sibling_ordering_does_not_hide_children() {
    let t = FixtureTree::new();
    t.pacman_pkg(
        "sib",
        "1-1",
        Some(1),
        &["opt/foo-x/", "opt/foo.d/", "opt/foo/bar"],
    );
    let idx = load_pacman(&t.env()).unwrap().unwrap();
    assert!(idx.owns_at_or_below(Path::new("/opt/foo")));
    assert!(!idx.owns(Path::new("/opt/foo")));

    let t = FixtureTree::new();
    t.pacman_pkg("sib", "1-1", Some(1), &["opt/foo-x/", "opt/foo.d/"]);
    let idx = load_pacman(&t.env()).unwrap().unwrap();
    assert!(!idx.owns_at_or_below(Path::new("/opt/foo")));
}

#[test]
fn trailing_and_duplicate_slashes_normalise() {
    let t = FixtureTree::new();
    t.pacman_pkg("sl", "1-1", Some(1), &["opt//foo//", "opt//foo//bin/x"]);
    let idx = load_pacman(&t.env()).unwrap().unwrap();
    assert!(idx.owns_dir(Path::new("/opt/foo")));
    assert!(idx.owns(Path::new("/opt/foo/bin/x")));
    assert!(idx.is_complete());
}

#[test]
fn bad_queries_are_unowned() {
    let t = FixtureTree::new();
    t.pacman_pkg("p", "1-1", Some(1), &["opt/foo/"]);
    let idx = load_pacman(&t.env()).unwrap().unwrap();
    for q in ["", "opt/foo", "foo"] {
        assert!(!idx.owns(Path::new(q)), "{q}");
        assert!(!idx.owns_at_or_below(Path::new(q)), "{q}");
    }
}
