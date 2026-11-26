//! Fixtures for the package-manager and system-wide tests: fake pacman and flatpak, root-only
//! trees, and the apps they hold.

use super::*;

// ---- full uninstall with one privileged batch (only ever inside the fixture) ----

/// A fake `pacman`: the simulation (`-Rs --print ...`) prints firefox and
/// two dependencies; the removal (`-Rn --noconfirm <names>`) logs its
/// arguments to `<root>/pacman-args` and deletes those packages' database
/// entries. It runs as the test user, inside the fixture.
pub(crate) const PACMAN_OK: &str = "#!/bin/sh\nroot=\"$(dirname \"$0\")/../..\"\ncase \"$1\" in\n-Rs) printf 'firefox 159.0.1-1\\ngn 0.2385.1-1\\nlibjuice 1.7.3-1\\n' ;;\n-Rn) echo \"$@\" >> \"$root/pacman-args\"; shift 2; for p in \"$@\"; do rm -rf \"$root\"/var/lib/pacman/local/\"$p\"-*; done ;;\n*) exit 9 ;;\nesac\n";

/// The simulation refuses, with an escape byte in its message.
pub(crate) const PACMAN_REFUSE: &str = "#!/bin/sh\nprintf ':: removing firefox breaks dependency \\033[1mfirefox\\033[0m required by foo\\n'\necho 'error: failed to prepare transaction (could not satisfy dependencies)' >&2\nexit 1\n";

/// Lists the fixture's Flatpak apps; `uninstall` logs its arguments to
/// `<root>/flatpak-args` instead of removing anything.
pub(crate) const FLATPAK_APPS_RECORD: &str = "#!/bin/sh\nif [ \"$1\" = uninstall ]; then echo \"$@\" >> \"$(dirname \"$0\")/../../flatpak-args\"; exit 0; fi\nprintf 'app.xemu.xemu\\txemu\\t0.8.136\\tuser\\t34.4 MB\\ncom.dosbox_x.DOSBox-X\\tDOSBox-X\\t2026.10.01\\tsystem\\t51.4 MB\\n'\n";

pub(crate) fn pacman_fixture(pacman: &str) -> FixtureTree {
    let t = fixture(FLATPAK_OK);
    t.pacman_pkg("gn", "0.2385.1-1", Some(1_000), &["usr/bin/gn"]);
    t.pacman_pkg("libjuice", "1.7.3-1", Some(1_000), &["usr/lib/libjuice.so"]);
    t.write_exec("/usr/bin/pacman", pacman.as_bytes());
    t.write("/home/user/.cache/firefox/blob", &[b'x'; 4000]);
    t
}

pub(crate) fn run_out(cmd: &mut Command) -> (i32, String, String) {
    let out = cmd.output().unwrap();
    (
        out.status.code().unwrap(),
        String::from_utf8(out.stdout).unwrap(),
        String::from_utf8(out.stderr).unwrap(),
    )
}

pub(crate) fn pkg_present(t: &FixtureTree, dir: &str) -> bool {
    real(t, &format!("/var/lib/pacman/local/{dir}/desc")).exists()
}

pub(crate) const MANUAL_PACMAN: &str =
    "sudo '/usr/bin/pacman' '-Rn' '--noconfirm' 'firefox' 'gn' 'libjuice'";

/// Whether the tests run as root (as in the distro containers): escalation
/// detection then reports running as root instead of run0/pkexec/sudo.
pub(crate) fn running_as_root() -> bool {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata("/proc/self").is_ok_and(|m| m.uid() == 0)
}

/// Like [`PACMAN_OK`], but any other package simulates as just itself.
pub(crate) const PACMAN_MULTI: &str = "#!/bin/sh\nroot=\"$(dirname \"$0\")/../..\"\ncase \"$1\" in\n-Rs) if [ \"$5\" = firefox ]; then printf 'firefox 159.0.1-1\\ngn 0.2385.1-1\\nlibjuice 1.7.3-1\\n'; else printf '%s 1.0-1\\n' \"$5\"; fi ;;\n-Rn) echo \"$@\" >> \"$root/pacman-args\"; shift 2; for p in \"$@\"; do rm -rf \"$root\"/var/lib/pacman/local/\"$p\"-*; done ;;\n*) exit 9 ;;\nesac\n";

pub(crate) fn two_pacman_apps() -> FixtureTree {
    let t = pacman_fixture(PACMAN_MULTI);
    t.pacman_pkg(
        "kate",
        "1.0-1",
        Some(1_000),
        &["usr/bin/kate", "usr/share/applications/kate.desktop"],
    );
    t.write_exec("/usr/bin/kate", b"");
    t.desktop(
        "/usr/share/applications/kate.desktop",
        "[Desktop Entry]\nType=Application\nName=Kate\nExec=/usr/bin/kate\n",
    );
    t.write("/home/user/.cache/kate/blob", b"k");
    t
}

// ---- system categories (`--system`) ----

pub(crate) const SYS_ORPHANS: [&str; 7] = [
    "orph1", "orph2", "orph3", "orph4", "orph5", "orph6", "orph7",
];

/// A fixture with every system tool as a recording script. Each one logs
/// its arguments to `<root>/<tool>-args` when it changes something. The
/// journal query first runs `JOURNAL_HOOK` (empty unless a test sets it).
pub(crate) fn system_fixture(journal_hook: &str) -> FixtureTree {
    let t = FixtureTree::new();
    t.write_exec(
        "/usr/bin/paccache",
        b"#!/bin/sh\nroot=\"$(dirname \"$0\")/../..\"\ncase \"$1\" in\n-dk2) printf '==> finished dry run: 1 candidates (disk space saved: 86.17 MiB)\\n' ;;\n-rk2) echo \"$@\" >> \"$root/paccache-args\" ;;\n*) exit 9 ;;\nesac\n",
    );
    t.write_exec(
        "/usr/bin/pacman",
        b"#!/bin/sh\nroot=\"$(dirname \"$0\")/../..\"\ncase \"$1\" in\n-Qdtq) printf 'orph1\\norph2\\norph3\\norph4\\norph5\\norph6\\norph7\\n' ;;\n-Rs) shift 4; for p in \"$@\"; do printf '%s 1.0-1\\n' \"$p\"; done ;;\n-Rn) echo \"$@\" >> \"$root/pacman-args\" ;;\n*) exit 9 ;;\nesac\n",
    );
    let journalctl = format!(
        "#!/bin/sh\nroot=\"$(dirname \"$0\")/../..\"\ncase \"$1\" in\n--disk-usage) {journal_hook}\nprintf 'Archived and active journals take up 719.1M in the file system.\\n' ;;\n--vacuum-size=500M) echo \"$@\" >> \"$root/journal-args\" ;;\n*) exit 9 ;;\nesac\n"
    );
    t.write_exec("/usr/bin/journalctl", journalctl.as_bytes());
    t.write_exec(
        "/usr/bin/systemd-tmpfiles",
        b"#!/bin/sh\necho \"$@\" >> \"$(dirname \"$0\")/../../tmpfiles-args\"\n",
    );
    t.write_exec(
        "/usr/bin/flatpak",
        b"#!/bin/sh\nroot=\"$(dirname \"$0\")/../..\"\nif [ \"$1\" = list ]; then case \"$*\" in *--runtime*) echo org.x.Platform/x86_64/23.08;; esac; exit 0; fi\n[ \"$1\" = --version ] && exit 0\necho \"$@\" >> \"$root/flatpak-args\"\n",
    );
    // `rm` logs, then removes the root-joined path the batch passes.
    t.write_exec(
        "/usr/bin/rm",
        b"#!/bin/sh\necho \"$@\" >> \"$(dirname \"$0\")/../../rm-args\"\nexec /usr/bin/rm \"$@\"\n",
    );
    t.mkdir("var/lib/systemd/coredump");
    let mut cur = t.dir.path().to_path_buf();
    for c in ["", "var", "lib", "systemd", "coredump"] {
        cur.push(c);
        std::fs::set_permissions(&cur, std::os::unix::fs::PermissionsExt::from_mode(0o755))
            .unwrap();
    }
    t.write(
        "/var/lib/systemd/coredump/core.foo.1000.abc.1.zst",
        &[7u8; 9000],
    );
    t
}

pub(crate) const CORE: &str = "/var/lib/systemd/coredump/core.foo.1000.abc.1.zst";

pub(crate) fn sh_runner(t: &FixtureTree) -> Command {
    let mut cmd = crumbs(t);
    cmd.env("CRUMBS_TEST_PRIV_RUNNER", "sh");
    cmd
}

pub(crate) fn recorded(t: &FixtureTree, file: &str) -> Option<String> {
    std::fs::read_to_string(real(t, file)).ok()
}

/// Swaps the coredump for a symlink to `<root>/precious` after the scan
/// and confirmation, before anything executes (a debug-only test hook).
pub(crate) fn swap_hook() -> String {
    format!("rm -f \"$CRUMBS_ROOT{CORE}\"; ln -s \"$CRUMBS_ROOT/precious\" \"$CRUMBS_ROOT{CORE}\"")
}

// ---- system leftovers, unmanaged removal, `orphans --system` ----

/// Makes `/`, `/etc` and `/opt` pass the system-tree checks (this user
/// stands in for root) and installs a logging `rm` for the root batch.
pub(crate) fn sys_ready(t: &FixtureTree) {
    t.mkdir("etc");
    t.mkdir("opt");
    for rel in ["", "etc", "opt"] {
        std::fs::set_permissions(
            t.dir.path().join(rel),
            std::os::unix::fs::PermissionsExt::from_mode(0o755),
        )
        .unwrap();
    }
    t.write_exec(
        "/usr/bin/rm",
        b"#!/bin/sh\necho \"$@\" >> \"$(dirname \"$0\")/../../rm-args\"\nexec /usr/bin/rm \"$@\"\n",
    );
    t.write_exec("/usr/bin/sh", b"#!/bin/sh\nexec /bin/sh \"$@\"\n");
    // The guard compares mount namespaces with the trusted `readlink`.
    t.write_exec(
        "/usr/bin/readlink",
        b"#!/bin/sh\nexec /usr/bin/readlink \"$@\"\n",
    );
    // The root batch refuses a tree with a mount point inside.
    t.write(
        "/proc/self/mountinfo",
        b"36 35 98:0 /mnt1 / rw,noatime master:1 - ext3 /dev/root rw\n",
    );
}

/// A package `inky` that owns only its launcher, with unowned `/opt/inky`
/// and `/etc/inky` beside it.
pub(crate) fn inky_pkg_fixture() -> FixtureTree {
    let t = FixtureTree::new();
    t.os_release("manjaro", "arch", "Manjaro Linux");
    t.pacman_pkg("coreutils", "9.5-1", Some(2_000_000), &["usr/bin/ls"]);
    t.pacman_pkg("inky", "1.0-1", Some(1_000), &["usr/bin/inky"]);
    t.write_exec("/usr/bin/inky", b"");
    t.write("/opt/inky/bin/inky", b"x");
    t.write("/etc/inky/inky.conf", b"x");
    sys_ready(&t);
    t
}

/// An unmanaged app `inky` (an unowned launcher running `/opt/inky/inky`)
/// with `/etc/inky` beside it; the package database is complete.
pub(crate) fn inky_unmanaged_fixture() -> FixtureTree {
    let t = FixtureTree::new();
    t.os_release("manjaro", "arch", "Manjaro Linux");
    t.pacman_pkg("coreutils", "9.5-1", Some(2_000_000), &["usr/bin/ls"]);
    t.write_exec("/opt/inky/inky", b"");
    t.write("/etc/inky/inky.conf", b"x");
    t.desktop(
        "/usr/share/applications/inky.desktop",
        "[Desktop Entry]\nType=Application\nName=Inky\nExec=/opt/inky/inky\n",
    );
    sys_ready(&t);
    t
}

pub(crate) fn old_app_fixture() -> FixtureTree {
    let t = inky_unmanaged_fixture();
    t.write("/opt/old-app/bin/x", b"x");
    t.write("/etc/old-app/x.conf", b"x");
    t
}
