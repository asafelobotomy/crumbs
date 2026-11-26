#![allow(clippy::unwrap_used, clippy::expect_used)]

// A release binary refuses CRUMBS_ROOT, so these tests would fail; before
// that refusal they ran against the real root and home.
#[cfg(not(debug_assertions))]
compile_error!("crumbs CLI tests must run in a debug build; they rely on CRUMBS_ROOT");

use assert_cmd::Command;
use crumbs_core::testing::FixtureTree;
use predicates::prelude::*;

const FLATPAK_OK: &str = "#!/bin/sh\nprintf 'app.xemu.xemu\\txemu\\t0.8.136\\tuser\\t34.4 MB\\ncom.dosbox_x.DOSBox-X\\tDOSBox-X\\t2026.10.01\\tsystem\\t51.4 MB\\ncom.obsproject.Studio\\tOBS Studio\\t33.0.0\\tsystem\\t593.5 MB\\n'\n";
const FLATPAK_FAIL: &str = "#!/bin/sh\necho boom >&2\nexit 1\n";

fn fixture(flatpak_script: &str) -> FixtureTree {
    let t = FixtureTree::new();
    t.os_release("manjaro", "arch", "Manjaro Linux");
    t.pacman_pkg(
        "firefox",
        "159.0.1-1",
        Some(300_000_000),
        &[
            "usr/bin/firefox",
            "usr/share/applications/",
            "usr/share/applications/firefox.desktop",
        ],
    );
    t.pacman_pkg("coreutils", "9.5-1", Some(2_000_000), &["usr/bin/ls"]);
    t.desktop(
        "/usr/share/applications/firefox.desktop",
        "[Desktop Entry]\nType=Application\nName=Firefox\nExec=/usr/bin/firefox %u\n",
    );
    t.write_exec("/usr/bin/firefox", b"");
    t.write_exec("/usr/bin/ls", b"");
    t.appimage("/home/user/Applications/Tool-1.0-x86_64.AppImage");
    t.write_exec("/opt/foo/foo", b"");
    t.desktop(
        "/usr/share/applications/foo.desktop",
        "[Desktop Entry]\nType=Application\nName=Zed Foo\nExec=/opt/foo/foo\n",
    );
    t.write_exec("/usr/bin/flatpak", flatpak_script.as_bytes());
    t
}

fn crumbs(t: &FixtureTree) -> Command {
    let mut cmd = assert_cmd::cargo::cargo_bin_cmd!("crumbs");
    cmd.env("CRUMBS_ROOT", t.dir.path());
    cmd.env_remove("NO_COLOR");
    // Only a test that asks for the fake privileged runner gets one.
    cmd.env_remove("CRUMBS_TEST_PRIV_RUNNER");
    cmd.env_remove("CRUMBS_ALLOW_REAL_ESCALATION");
    cmd
}

fn stdout_json(cmd: &mut Command) -> serde_json::Value {
    let out = cmd.output().unwrap();
    serde_json::from_slice(&out.stdout).unwrap()
}

fn regex_escape(s: &str) -> String {
    s.chars()
        .flat_map(|c| {
            if c.is_ascii_alphanumeric() || c == '/' || c == '_' {
                vec![c]
            } else {
                vec!['\\', c]
            }
        })
        .collect()
}

fn with_leftovers(t: &FixtureTree) {
    t.write("/home/user/.config/Tool/settings.json", &[b'x'; 5000]);
    t.write("/home/user/.cache/Tool/blob", &[b'y'; 9000]);
}

fn with_orphans(t: &FixtureTree) {
    t.write("/home/user/.thunderbird/profiles.ini", b"x");
    t.mkdir("/home/user/.var/app/org.gone.App");
    t.mkdir("/home/user/.config/zzquux-thing");
    // Orphans modified in the last 30 days are capped at low: make them old.
    let old = std::time::SystemTime::now() - std::time::Duration::from_secs(60 * 86_400);
    for d in [
        "/home/user/.thunderbird",
        "/home/user/.var/app/org.gone.App",
        "/home/user/.config/zzquux-thing",
    ] {
        std::fs::File::open(t.mkdir(d))
            .unwrap()
            .set_modified(old)
            .unwrap();
    }
}

/// What the fixture's filesystem allocates to an empty directory: nothing
/// on btrfs or tmpfs, a 4 KiB block on ext4 (as on CI runners). Sizes
/// count it, since deleting the directory frees it.
fn dir_block(t: &FixtureTree) -> u64 {
    use std::os::unix::fs::MetadataExt;
    let probe = t.dir.path().join(".dir-block-probe");
    std::fs::create_dir(&probe).unwrap();
    let bytes = std::fs::metadata(&probe).unwrap().blocks() * 512;
    std::fs::remove_dir(&probe).unwrap();
    bytes
}

/// A size as crumbs prints it, for the small sizes these tests use.
fn size_text(bytes: u64) -> String {
    if bytes == 0 {
        "0 B".to_string()
    } else {
        format!("{:.1} KiB", bytes as f64 / 1024.0)
    }
}

/// Rewrites what this filesystem reports to what one without directory
/// blocks reports, so a snapshot holds on both. Each row is (text marking
/// the line, bytes on a filesystem without directory blocks, directories
/// counted in it); the size keeps its column width.
fn without_dir_blocks(t: &FixtureTree, text: String, rows: &[(&str, u64, u64)]) -> String {
    let block = dir_block(t);
    if block == 0 {
        return text;
    }
    let mut out = String::new();
    for line in text.split_inclusive('\n') {
        let mut line = line.to_string();
        if let Some((_, bytes, dirs)) = rows.iter().find(|(m, _, _)| line.contains(m)) {
            let actual = size_text(bytes + dirs * block);
            // Never longer than `actual`: pad it to keep the columns.
            let wanted = format!("{:<w$}", size_text(*bytes), w = actual.len());
            line = line.replacen(&actual, &wanted, 1);
            let end = if line.ends_with('\n') { "\n" } else { "" };
            line = format!("{}{end}", line.trim_end());
        }
        out.push_str(&line);
    }
    out
}

fn snapshot_text(t: &FixtureTree, args: &[&str]) -> String {
    let out = crumbs(t).args(args).output().unwrap();
    assert_eq!(out.status.code(), Some(0));
    let text = String::from_utf8(out.stdout).unwrap();
    text.replace(&t.dir.path().to_string_lossy().into_owned(), "[ROOT]")
}

// ---- uninstall, orphans --clean, history, restore (all inside the fixture) ----

fn real(t: &FixtureTree, in_system: &str) -> std::path::PathBuf {
    t.dir.path().join(in_system.trim_start_matches('/'))
}

fn trash_files(t: &FixtureTree) -> Vec<String> {
    let dir = real(t, "/home/user/.local/share/Trash/files");
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .map(|d| {
            d.flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

fn gutcheck(t: &FixtureTree) {
    t.appimage("/home/user/Applications/GutCheck-0.19.0-x86_64.AppImage");
    t.write("/home/user/.config/GutCheck/settings.json", &[b'x'; 5000]);
    t.write("/home/user/.cache/GutCheck/blob", &[b'y'; 9000]);
}

fn history_json(t: &FixtureTree) -> serde_json::Value {
    stdout_json(crumbs(t).args(["history", "--json"]))
}

fn json_error(t: &FixtureTree, args: &[&str], code: i32) -> serde_json::Value {
    let out = crumbs(t).args(args).output().unwrap();
    assert_eq!(out.status.code(), Some(code));
    assert!(!out.stderr.is_empty(), "human text stays on stderr");
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["schema_version"], 1);
    let errors = v["errors"].as_array().unwrap();
    assert!(errors.iter().any(|e| e["source"] == "cli"), "{v}");
    v
}

fn gutcheck_launcher(t: &FixtureTree) {
    gutcheck(t);
    t.desktop(
        "/home/user/.local/share/applications/appofholding-gutcheck.desktop",
        "[Desktop Entry]\nType=Application\nName=GutCheck\nExec=/home/user/Applications/GutCheck-0.19.0-x86_64.AppImage\nIcon=appofholding-gutcheck\n",
    );
    t.write(
        "/home/user/.local/share/icons/hicolor/512x512/apps/appofholding-gutcheck.png",
        b"png",
    );
}

// ---- M3 task 4: CLI consistency ----

fn bad_config(t: &FixtureTree) {
    t.write("/home/user/.config/crumbs/config.toml", b"protect = [\n");
}

fn batch_ids(t: &FixtureTree) -> Vec<String> {
    history_json(t)["data"]["batches"]
        .as_array()
        .unwrap()
        .iter()
        .map(|b| b["batch"].as_str().unwrap().to_string())
        .collect()
}

fn json_error_free(t: &FixtureTree, args: &[&str], code: i32) -> serde_json::Value {
    let out = crumbs(t).args(args).output().unwrap();
    assert_eq!(out.status.code(), Some(code));
    serde_json::from_slice(&out.stdout).unwrap()
}

// ---- scan ----

fn scan_fixture(flatpak_script: &str) -> FixtureTree {
    let t = fixture(flatpak_script);
    t.write("/home/user/.cache/foo/blob", &[1u8; 3000]);
    t.write("/home/user/.cache/thumbnails/a.png", &[1u8; 2000]);
    t.write("/home/user/.local/share/Trash/files/old.txt", &[1u8; 5000]);
    t.write(
        "/home/user/.local/share/Trash/info/old.txt.trashinfo",
        b"[Trash Info]\nPath=/home/user/docs/old.txt\nDeletionDate=2026-01-01T00:00:00\n",
    );
    t.write("/home/user/.cargo/registry/cache/crate.bin", &[1u8; 7000]);
    t
}

/// Every file and symlink under the tree with its bytes.
fn tree_state(t: &FixtureTree) -> std::collections::BTreeMap<String, Vec<u8>> {
    fn walk(dir: &std::path::Path, root: &std::path::Path, out: &mut Vec<(String, Vec<u8>)>) {
        let mut entries: Vec<_> = std::fs::read_dir(dir).unwrap().flatten().collect();
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            let p = e.path();
            let rel = p.strip_prefix(root).unwrap().to_string_lossy().into_owned();
            let ft = e.file_type().unwrap();
            if ft.is_symlink() {
                let target = std::fs::read_link(&p).unwrap();
                out.push((rel, target.to_string_lossy().into_owned().into_bytes()));
            } else if ft.is_dir() {
                out.push((format!("{rel}/"), Vec::new()));
                walk(&p, root, out);
            } else {
                out.push((rel, std::fs::read(&p).unwrap()));
            }
        }
    }
    let mut v = Vec::new();
    walk(t.dir.path(), t.dir.path(), &mut v);
    v.into_iter().collect()
}

// ---- clean (only ever inside the fixture) ----

/// Lists apps and runtimes; `uninstall` appends its args to
/// `<root>/flatpak-args` (the child gets no `CRUMBS_ROOT`) instead of removing anything.
const FLATPAK_RECORD: &str = "#!/bin/sh\nif [ \"$1\" = uninstall ]; then echo \"$@\" >> \"$(dirname \"$0\")/../../flatpak-args\"; exit 0; fi\nprintf 'app.xemu.xemu\\txemu\\t0.8.136\\tuser\\t34.4 MB\\n'\n";
/// The app listing works; only the runtime listing (the `flatpak-unused`
/// scanner) fails.
const FLATPAK_RUNTIME_FAIL: &str = "#!/bin/sh\ncase \"$*\" in *--runtime*) echo boom >&2; exit 1;; esac\nprintf 'app.xemu.xemu\\txemu\\t0.8.136\\tuser\\t34.4 MB\\n'\n";

/// Days `run_clean` moves crumbs' clock ahead: every fixture entry then
/// has an old ctime (which cannot be set), so mtimes decide what is old.
const CLOCK_SHIFT_DAYS: u64 = 100;

fn run_clean(t: &FixtureTree, args: &[&str]) -> (i32, String, String) {
    let out = crumbs(t)
        .env("CRUMBS_TEST_CLOCK_SHIFT_DAYS", CLOCK_SHIFT_DAYS.to_string())
        .arg("clean")
        .args(args)
        .output()
        .unwrap();
    (
        out.status.code().unwrap(),
        String::from_utf8(out.stdout).unwrap(),
        String::from_utf8(out.stderr).unwrap(),
    )
}

/// A file of the user's directly in the fixture's `/tmp`, 30 days old by
/// the clock of [`run_clean`].
fn old_temp(t: &FixtureTree, name: &str) {
    let p = t.write(&format!("/tmp/{name}"), &[1u8; 4000]);
    let old = std::time::SystemTime::now()
        + std::time::Duration::from_secs(CLOCK_SHIFT_DAYS * 86_400)
        - std::time::Duration::from_secs(30 * 86_400);
    std::fs::File::options()
        .write(true)
        .open(&p)
        .unwrap()
        .set_modified(old)
        .unwrap();
}

/// A flatpak 1.x whose `uninstall --unused` without `-y` lists one unused
/// runtime and declines, as flatpak does when stdout is not a terminal.
/// Every `uninstall` it gets is recorded in `<root>/flatpak-args`.
const FLATPAK_PROBED: &str = "#!/bin/sh\nroot=\"$(dirname \"$0\")/../..\"\ncase \"$*\" in\n--version) echo 'Flatpak 1.18.4' ;;\nuninstall*) echo \"$@\" >> \"$root/flatpak-args\"; case \"$*\" in *-y*) ;; *) printf ' 1.     org.gnome.Platform   45   r\\nProceed with these changes to the user installation? [Y/n]: n\\n'; exit 1 ;; esac ;;\n*runtime*) echo org.gnome.Platform/x86_64/45 ;;\n*) printf 'app.xemu.xemu\\txemu\\t0.8.136\\tuser\\t34.4 MB\\n' ;;\nesac\n";

/// A Firefox process in the fixture's `/proc`.
fn firefox_running(t: &FixtureTree) {
    t.symlink("/proc/79/exe", "/usr/bin/firefox");
}

const EVIL_APPIMAGE: &str = "/home/user/Applications/Ev\x1b[2Jil\rX-1.0-x86_64.AppImage";

/// The fixture plus an AppImage whose file name holds ESC and CR, and its id.
fn evil_fixture() -> (FixtureTree, String) {
    let t = fixture(FLATPAK_OK);
    t.appimage(EVIL_APPIMAGE);
    let j = stdout_json(crumbs(&t).args(["apps", "list", "--json"]));
    let id = j["data"]["apps"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["id"].as_str().unwrap().to_string())
        .find(|id| id.contains('\x1b'))
        .expect("evil app listed");
    (t, id)
}

fn assert_clean(out: &std::process::Output) {
    for (name, bytes) in [("stdout", &out.stdout), ("stderr", &out.stderr)] {
        let text = String::from_utf8_lossy(bytes);
        assert!(!text.contains('\r'), "{name} has CR: {text:?}");
        assert!(!text.contains('\x1b'), "{name} has ESC: {text:?}");
    }
}

fn assert_shows_replacement(out: &std::process::Output) {
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains('\u{FFFD}'), "{text:?}");
}

// ---- full uninstall with one privileged batch (only ever inside the fixture) ----

/// A fake `pacman`: the simulation (`-Rs --print ...`) prints firefox and
/// two dependencies; the removal (`-Rn --noconfirm <names>`) logs its
/// arguments to `<root>/pacman-args` and deletes those packages' database
/// entries. It runs as the test user, inside the fixture.
const PACMAN_OK: &str = "#!/bin/sh\nroot=\"$(dirname \"$0\")/../..\"\ncase \"$1\" in\n-Rs) printf 'firefox 159.0.1-1\\ngn 0.2385.1-1\\nlibjuice 1.7.3-1\\n' ;;\n-Rn) echo \"$@\" >> \"$root/pacman-args\"; shift 2; for p in \"$@\"; do rm -rf \"$root\"/var/lib/pacman/local/\"$p\"-*; done ;;\n*) exit 9 ;;\nesac\n";
/// The simulation refuses, with an escape byte in its message.
const PACMAN_REFUSE: &str = "#!/bin/sh\nprintf ':: removing firefox breaks dependency \\033[1mfirefox\\033[0m required by foo\\n'\necho 'error: failed to prepare transaction (could not satisfy dependencies)' >&2\nexit 1\n";
/// Lists the fixture's Flatpak apps; `uninstall` logs its arguments to
/// `<root>/flatpak-args` instead of removing anything.
const FLATPAK_APPS_RECORD: &str = "#!/bin/sh\nif [ \"$1\" = uninstall ]; then echo \"$@\" >> \"$(dirname \"$0\")/../../flatpak-args\"; exit 0; fi\nprintf 'app.xemu.xemu\\txemu\\t0.8.136\\tuser\\t34.4 MB\\ncom.dosbox_x.DOSBox-X\\tDOSBox-X\\t2026.10.01\\tsystem\\t51.4 MB\\n'\n";

fn pacman_fixture(pacman: &str) -> FixtureTree {
    let t = fixture(FLATPAK_OK);
    t.pacman_pkg("gn", "0.2385.1-1", Some(1_000), &["usr/bin/gn"]);
    t.pacman_pkg("libjuice", "1.7.3-1", Some(1_000), &["usr/lib/libjuice.so"]);
    t.write_exec("/usr/bin/pacman", pacman.as_bytes());
    t.write("/home/user/.cache/firefox/blob", &[b'x'; 4000]);
    t
}

fn run_out(cmd: &mut Command) -> (i32, String, String) {
    let out = cmd.output().unwrap();
    (
        out.status.code().unwrap(),
        String::from_utf8(out.stdout).unwrap(),
        String::from_utf8(out.stderr).unwrap(),
    )
}

fn pkg_present(t: &FixtureTree, dir: &str) -> bool {
    real(t, &format!("/var/lib/pacman/local/{dir}/desc")).exists()
}

const MANUAL_PACMAN: &str = "sudo '/usr/bin/pacman' '-Rn' '--noconfirm' 'firefox' 'gn' 'libjuice'";

/// Whether the tests run as root (as in the distro containers): escalation
/// detection then reports running as root instead of run0/pkexec/sudo.
fn running_as_root() -> bool {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata("/proc/self").is_ok_and(|m| m.uid() == 0)
}

/// Like [`PACMAN_OK`], but any other package simulates as just itself.
const PACMAN_MULTI: &str = "#!/bin/sh\nroot=\"$(dirname \"$0\")/../..\"\ncase \"$1\" in\n-Rs) if [ \"$5\" = firefox ]; then printf 'firefox 159.0.1-1\\ngn 0.2385.1-1\\nlibjuice 1.7.3-1\\n'; else printf '%s 1.0-1\\n' \"$5\"; fi ;;\n-Rn) echo \"$@\" >> \"$root/pacman-args\"; shift 2; for p in \"$@\"; do rm -rf \"$root\"/var/lib/pacman/local/\"$p\"-*; done ;;\n*) exit 9 ;;\nesac\n";

fn two_pacman_apps() -> FixtureTree {
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

const SYS_ORPHANS: [&str; 7] = [
    "orph1", "orph2", "orph3", "orph4", "orph5", "orph6", "orph7",
];

/// A fixture with every system tool as a recording script. Each one logs
/// its arguments to `<root>/<tool>-args` when it changes something. The
/// journal query first runs `JOURNAL_HOOK` (empty unless a test sets it).
fn system_fixture(journal_hook: &str) -> FixtureTree {
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

const CORE: &str = "/var/lib/systemd/coredump/core.foo.1000.abc.1.zst";

fn sh_runner(t: &FixtureTree) -> Command {
    let mut cmd = crumbs(t);
    cmd.env("CRUMBS_TEST_PRIV_RUNNER", "sh");
    cmd
}

fn recorded(t: &FixtureTree, file: &str) -> Option<String> {
    std::fs::read_to_string(real(t, file)).ok()
}

/// Swaps the coredump for a symlink to `<root>/precious` after the scan
/// and confirmation, before anything executes (a debug-only test hook).
fn swap_hook() -> String {
    format!("rm -f \"$CRUMBS_ROOT{CORE}\"; ln -s \"$CRUMBS_ROOT/precious\" \"$CRUMBS_ROOT{CORE}\"")
}

// ---- system leftovers, unmanaged removal, `orphans --system` ----

/// Makes `/`, `/etc` and `/opt` pass the system-tree checks (this user
/// stands in for root) and installs a logging `rm` for the root batch.
fn sys_ready(t: &FixtureTree) {
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
fn inky_pkg_fixture() -> FixtureTree {
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
fn inky_unmanaged_fixture() -> FixtureTree {
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

fn old_app_fixture() -> FixtureTree {
    let t = inky_unmanaged_fixture();
    t.write("/opt/old-app/bin/x", b"x");
    t.write("/etc/old-app/x.conf", b"x");
    t
}

mod apps_doctor;
mod clean;
mod clean_2;
mod doctor_flatpak;
mod named_system;
mod preview_source;
mod restore_dry;
mod scan_clean;
mod uninstall;
mod uninstall_2;
mod uninstall_history;
