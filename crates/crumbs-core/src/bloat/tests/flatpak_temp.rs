use super::*;

#[test]
fn flatpak_browser_cache_found() {
    let t = FixtureTree::new();
    t.write_exec("usr/bin/flatpak", b"#!/bin/sh\n");
    let ff = "home/user/.var/app/org.mozilla.firefox/cache/mozilla/firefox/xy.default";
    t.write(&format!("{ff}/cache2/x"), b"x");
    let br = "home/user/.var/app/com.brave.Browser/cache/BraveSoftware/Brave-Browser/Default";
    t.write(&format!("{br}/GPUCache/x"), b"x");
    // Another app's pattern under a browser's Flatpak id is not matched.
    t.write(
        "home/user/.var/app/org.mozilla.firefox/cache/chromium/Default/Cache/x",
        b"x",
    );
    let ps = CmdOutput {
        status: 0,
        stdout: "com.brave.Browser\norg.other.App\n".into(),
        stderr: String::new(),
        timed_out: false,
    };
    let runner = FakeRunner::new().on("flatpak", &["ps", "--columns=application"], ps);
    let items = scan_one(&t, &Config::default(), &runner, Category::BrowserCache).unwrap();
    let mut got = paths(&items);
    got.sort();
    assert_eq!(
        got,
        [
            PathBuf::from(format!("/{br}/GPUCache")),
            PathBuf::from(format!("/{ff}/cache2")),
        ]
    );
    let brave = items.iter().find(|i| i.label.starts_with("Brave")).unwrap();
    assert_eq!(brave.label, "Brave (Brave-Browser/Default) GPUCache");
    assert!(!brave.preselected);
    assert_eq!(brave.note.as_deref(), Some("Brave is running"));
    let fox = items
        .iter()
        .find(|i| i.label.starts_with("Firefox"))
        .unwrap();
    assert_eq!(fox.label, "Firefox (xy.default) cache2");
    assert!(fox.preselected && fox.note.is_none());
    check_browser(&t, &items);
    // Asked once, however many Flatpak items there are.
    assert_eq!(runner.calls(), ["flatpak ps --columns=application"]);

    // `flatpak ps` failing, or no trusted flatpak: fail closed.
    let failing = FakeRunner::new().on(
        "flatpak",
        &["ps", "--columns=application"],
        CmdOutput {
            status: 1,
            stdout: String::new(),
            stderr: "boom".into(),
            timed_out: false,
        },
    );
    for runner in [FakeRunner::new(), failing] {
        let items = scan_one(&t, &Config::default(), &runner, Category::BrowserCache).unwrap();
        assert_eq!(items.len(), 2);
        for i in &items {
            assert!(!i.preselected);
            let browser = i.label.split(' ').next().unwrap();
            assert!(i.label.ends_with(" cache2") || i.label.ends_with(" GPUCache"));
            assert_eq!(
                i.note.as_deref(),
                Some(format!("could not check whether {browser} is running").as_str())
            );
        }
    }
    fs::remove_file(t.dir.path().join("usr/bin/flatpak")).unwrap();
    let runner = FakeRunner::new();
    let items = scan_one(&t, &Config::default(), &runner, Category::BrowserCache).unwrap();
    assert!(items.iter().all(|i| !i.preselected));
    assert!(runner.calls().is_empty());
}

#[test]
fn temp_rules() {
    if crate::testing::running_as_root() {
        eprintln!("skipped: temp cleanup is not offered as root");
        return;
    }
    use rustix::fs::{CWD, FileType, Mode, mknodat};
    let t = FixtureTree::new();
    let tmp = t.mkdir("tmp");
    let sock = tmp.join("sock");
    let _listener = std::os::unix::net::UnixListener::bind(&sock).unwrap();
    let fifo = tmp.join("fifo");
    mknodat(CWD, &fifo, FileType::Fifo, Mode::from_raw_mode(0o600), 0).unwrap();
    let mut old: Vec<PathBuf> = vec![sock, fifo];
    for name in [
        ".X11-unix",
        ".ICE-unix",
        ".font-unix",
        "tmux-1000",
        "ssh-XXXXabc",
        "systemd-private-abc-colord.service-x",
        "pulse-abc",
    ] {
        old.push(t.write(&format!("tmp/{name}/f"), b"x"));
        old.push(tmp.join(name));
    }
    for name in ["krb5cc_1000", ".s.PGSQL.5432", ".X0-lock"] {
        old.push(t.write(&format!("tmp/{name}"), b"x"));
    }
    age_nofollow(&t.write("tmp/fresh", b"x"), 0);
    age_nofollow(&t.write("tmp/olddir-fresh/sub/new", b"x"), 0);
    old.push(tmp.join("olddir-fresh/sub"));
    old.push(tmp.join("olddir-fresh"));
    old.push(t.write("tmp/oldfile", b"x"));
    old.push(t.write("tmp/olddir-old/sub/a", b"x"));
    old.push(t.write("tmp/olddir-old/b", b"x"));
    old.push(tmp.join("olddir-old/sub"));
    old.push(tmp.join("olddir-old"));
    // A dir holding a socket may be live IPC: skipped.
    let inner = tmp.join("olddir-sock");
    fs::create_dir(&inner).unwrap();
    let _l2 = std::os::unix::net::UnixListener::bind(inner.join("s")).unwrap();
    old.push(inner.join("s"));
    old.push(inner);
    // An old symlink is never a candidate.
    t.symlink("tmp/oldlink", "/tmp/oldfile");
    old.push(tmp.join("oldlink"));
    for p in &old {
        age_nofollow(p, 30);
    }
    let items = scan_one(&t, &Config::default(), &FakeRunner::new(), Category::Temp).unwrap();
    assert_eq!(
        paths(&items),
        [
            PathBuf::from("/tmp/olddir-old"),
            PathBuf::from("/tmp/oldfile")
        ]
    );
    assert_eq!(labels(&items), ["/tmp/olddir-old", "/tmp/oldfile"]);
    check_temp(&t, &items);

    // The age threshold comes from the config.
    let cfg = Config {
        temp_age_days: 60,
        ..Config::default()
    };
    assert!(
        scan_one(&t, &cfg, &FakeRunner::new(), Category::Temp)
            .unwrap()
            .is_empty()
    );
    // Excluded paths are never listed.
    let cfg = Config {
        exclude: vec!["/tmp/oldfile".into()],
        ..Config::default()
    };
    assert_eq!(
        paths(&scan_one(&t, &cfg, &FakeRunner::new(), Category::Temp).unwrap()),
        [PathBuf::from("/tmp/olddir-old")]
    );
}

#[test]
fn temp_dir_over_cap_skipped() {
    if crate::testing::running_as_root() {
        eprintln!("skipped: temp cleanup is not offered as root");
        return;
    }
    let t = FixtureTree::new();
    let big = t.mkdir("tmp/big");
    for i in 0..10_001 {
        let p = big.join(format!("f{i}"));
        fs::write(&p, b"").unwrap();
        age_nofollow(&p, 30);
    }
    age_nofollow(&big, 30);
    let at_cap = t.mkdir("tmp/atcap");
    for i in 0..10_000 {
        let p = at_cap.join(format!("f{i}"));
        fs::write(&p, b"").unwrap();
        age_nofollow(&p, 30);
    }
    age_nofollow(&at_cap, 30);
    let items = scan_one(&t, &Config::default(), &FakeRunner::new(), Category::Temp).unwrap();
    assert_eq!(paths(&items), [PathBuf::from("/tmp/atcap")]);
}

#[test]
fn temp_owner_rule() {
    if crate::testing::running_as_root() {
        eprintln!("skipped: temp cleanup is not offered as root");
        return;
    }
    use std::os::unix::fs::MetadataExt;
    let t = FixtureTree::new();
    let f = t.write("tmp/f", b"x");
    let d = t.mkdir("tmp/d");
    t.symlink("tmp/l", "/tmp/f");
    let fm = fs::symlink_metadata(&f).unwrap();
    let dm = fs::symlink_metadata(&d).unwrap();
    let lm = fs::symlink_metadata(t.dir.path().join("tmp/l")).unwrap();
    let me = fm.uid();
    let now = SystemTime::now();
    let week = Duration::from_secs(7 * 86_400);
    let old = now - Duration::from_secs(30 * 86_400);
    let ok = |name: &str, owner: u32, ft: fs::FileType, mtime: SystemTime| {
        let st = super::super::temp::EntryStat {
            owner_uid: owner,
            file_type: ft,
            changed: mtime,
        };
        super::super::temp::temp_candidate(std::ffi::OsStr::new(name), &st, me, now, week)
    };
    assert!(ok("a", me, fm.file_type(), old));
    assert!(ok("a", me, dm.file_type(), old));
    // Someone else's file or dir.
    assert!(!ok("a", me.wrapping_add(1), fm.file_type(), old));
    assert!(!ok("a", 0, dm.file_type(), old));
    assert!(!ok("a", me, lm.file_type(), old));
    assert!(!ok("a", me, fm.file_type(), now));
    // mtime in the future is not old.
    assert!(!ok("a", me, fm.file_type(), now + week));
    assert!(!ok("tmux-1000", me, dm.file_type(), old));
    assert!(!ok("xauth_XYZ", me, fm.file_type(), old));
}

#[test]
fn temp_extra_prefixes_kept() {
    if crate::testing::running_as_root() {
        eprintln!("skipped: temp cleanup is not offered as root");
        return;
    }
    let t = FixtureTree::new();
    let tmp = t.mkdir("tmp");
    let mut old = vec![];
    for name in [
        "xauth_abc",
        ".xauth123",
        "serverauth.x",
        "sddm-auth",
        "gdm-x",
        "lightdm-x",
        "kdeinit5__0",
        "plasma-csd-x",
        "dbus-abc",
        ".wine-1000",
        "nvidia-x",
        "mozilla_user0",
        "vscode-ipc",
        "pyright-x",
        "jetbrains-x",
    ] {
        old.push(t.write(&format!("tmp/{name}"), b"x"));
    }
    old.push(t.write("tmp/plain/sub/f", b"x"));
    old.push(tmp.join("plain/sub"));
    old.push(tmp.join("plain"));
    for p in &old {
        age_nofollow(p, 30);
    }
    let cfg = Config::default();
    let items = scan_one(&t, &cfg, &FakeRunner::new(), Category::Temp).unwrap();
    assert_eq!(paths(&items), [PathBuf::from("/tmp/plain")]);
    super::super::temp::still_stale(&t.env(), &cfg, Path::new("/tmp/plain"), test_now()).unwrap();
}

#[test]
fn temp_open_files_kept() {
    if crate::testing::running_as_root() {
        eprintln!("skipped: temp cleanup is not offered as root");
        return;
    }
    let t = FixtureTree::new();
    let tmp = t.mkdir("tmp");
    let lock = t.write("tmp/app.lock", b"x");
    let mapped = t.write("tmp/mapdir/lib.so", b"x");
    let cwd = t.mkdir("tmp/shell-here");
    let free = t.write("tmp/free", b"x");
    for p in [&lock, &mapped, &tmp.join("mapdir"), &cwd, &free] {
        age_nofollow(p, 30);
    }
    t.symlink("proc/50/fd/3", lock.to_str().unwrap());
    t.symlink("proc/50/fd/4", "/nonexistent/ignored");
    t.symlink("proc/52/cwd", cwd.to_str().unwrap());
    let m = fs::metadata(&mapped).unwrap();
    use std::os::unix::fs::MetadataExt;
    let line = format!(
        "7f00-7f10 r-xp 00000000 {:02x}:{:02x} {} /tmp/mapdir/lib.so\n00400000-00401000 r--p 00000000 00:00 0 [heap]\n",
        rustix::fs::major(m.dev()),
        rustix::fs::minor(m.dev()),
        m.ino()
    );
    t.write("proc/51/maps", line.as_bytes());
    let items = scan_one(&t, &Config::default(), &FakeRunner::new(), Category::Temp).unwrap();
    assert_eq!(paths(&items), [PathBuf::from("/tmp/free")]);
    let env = t.env();
    let err = super::super::temp::still_stale(
        &env,
        &Config::default(),
        Path::new("/tmp/app.lock"),
        test_now(),
    )
    .unwrap_err();
    assert_eq!(err, "is open in a running process");
}
