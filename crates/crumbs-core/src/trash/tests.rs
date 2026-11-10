use super::*;
use crate::testing::FixtureTree;
use std::collections::HashMap;

fn real(env: &Env, p: &str) -> PathBuf {
    env.sys(p)
}

#[test]
fn trash_writes_info_and_moves() {
    let t = FixtureTree::new();
    t.write("home/user/.config/My App/x.conf", b"hi");
    let env = t.env();
    let got = move_to_trash(&env, Path::new("/home/user/.config/My App")).unwrap();
    assert_eq!(got.original, Path::new("/home/user/.config/My App"));
    assert_eq!(
        got.files_path,
        Path::new("/home/user/.local/share/Trash/files/My App")
    );
    assert_eq!(
        got.info_path,
        Path::new("/home/user/.local/share/Trash/info/My App.trashinfo")
    );
    assert!(!real(&env, "/home/user/.config/My App").exists());
    assert!(env.sys_path(&got.files_path).join("x.conf").exists());
    let info = fs::read_to_string(env.sys_path(&got.info_path)).unwrap();
    let mut lines = info.lines();
    assert_eq!(lines.next(), Some("[Trash Info]"));
    assert_eq!(lines.next(), Some("Path=/home/user/.config/My%20App"));
    let date = lines.next().unwrap().strip_prefix("DeletionDate=").unwrap();
    assert_eq!(date.len(), 19);
    assert_eq!(&date[10..11], "T");
    assert!(info.ends_with('\n'));
}

#[test]
fn trash_name_collision() {
    let t = FixtureTree::new();
    t.write("home/user/.config/config/a", b"1");
    t.write("home/user/.local/share/config/b", b"2");
    let env = t.env();
    let a = move_to_trash(&env, Path::new("/home/user/.config/config")).unwrap();
    let b = move_to_trash(&env, Path::new("/home/user/.local/share/config")).unwrap();
    assert!(a.files_path.ends_with("files/config"));
    assert!(b.files_path.ends_with("files/config.2"));
    assert!(b.info_path.ends_with("info/config.2.trashinfo"));
}

#[test]
fn trash_skips_stale_info() {
    let t = FixtureTree::new();
    t.write("home/user/foo", b"x");
    let env = t.env();
    let info = env.data_home.join("Trash/info");
    mkdir_private(&info).unwrap();
    fs::write(info.join("foo.trashinfo"), b"stale").unwrap();
    let got = move_to_trash(&env, Path::new("/home/user/foo")).unwrap();
    assert!(got.files_path.ends_with("files/foo.2"));
}

#[test]
fn trash_moves_symlink_not_target() {
    let t = FixtureTree::new();
    t.write("home/user/target/keep", b"x");
    t.symlink("home/user/link", "target");
    let env = t.env();
    let got = move_to_trash(&env, Path::new("/home/user/link")).unwrap();
    assert!(t.dir.path().join("home/user/target/keep").exists());
    let moved = env.sys_path(&got.files_path);
    assert!(
        fs::symlink_metadata(moved)
            .unwrap()
            .file_type()
            .is_symlink()
    );
}

#[test]
fn trash_refuses_non_canonical() {
    let t = FixtureTree::new();
    t.write("home/user/a/b", b"x");
    let env = t.env();
    assert!(move_to_trash(&env, Path::new("/home/user/a/../a/b")).is_err());
    assert!(real(&env, "/home/user/a/b").exists());
}

#[test]
fn trash_failure_leaves_no_info() {
    let t = FixtureTree::new();
    let env = t.env();
    assert!(move_to_trash(&env, Path::new("/home/user/missing")).is_err());
    let info = env.data_home.join("Trash/info");
    assert_eq!(fs::read_dir(info).unwrap().count(), 0);
}

#[test]
fn restore_round_trip() {
    let t = FixtureTree::new();
    t.write("home/user/.config/app/x", b"data");
    let env = t.env();
    let orig = Path::new("/home/user/.config/app");
    let got = move_to_trash(&env, orig).unwrap();
    restore_from_trash(&env, &got).unwrap();
    assert_eq!(
        fs::read(real(&env, "/home/user/.config/app/x")).unwrap(),
        b"data"
    );
    assert!(!env.sys_path(&got.info_path).exists());
    assert!(!env.sys_path(&got.files_path).exists());
}

#[test]
fn restore_recreates_parents() {
    let t = FixtureTree::new();
    t.write("home/user/deep/er/app", b"d");
    let env = t.env();
    let got = move_to_trash(&env, Path::new("/home/user/deep/er/app")).unwrap();
    fs::remove_dir_all(real(&env, "/home/user/deep")).unwrap();
    restore_from_trash(&env, &got).unwrap();
    assert!(real(&env, "/home/user/deep/er/app").exists());
}

#[test]
fn restore_refuses_occupied() {
    let t = FixtureTree::new();
    t.write("home/user/app", b"old");
    let env = t.env();
    let got = move_to_trash(&env, Path::new("/home/user/app")).unwrap();
    t.write("home/user/app", b"new");
    match restore_from_trash(&env, &got) {
        Err(CoreError::Io { source, .. }) => {
            assert_eq!(source.kind(), io::ErrorKind::AlreadyExists)
        }
        other => panic!("expected AlreadyExists, got {other:?}"),
    }
    assert_eq!(fs::read(real(&env, "/home/user/app")).unwrap(), b"new");
    assert!(env.sys_path(&got.files_path).exists());
    assert!(env.sys_path(&got.info_path).exists());
}

#[test]
fn restore_refuses_dangling_symlink_occupant() {
    let t = FixtureTree::new();
    t.write("home/user/app", b"old");
    let env = t.env();
    let got = move_to_trash(&env, Path::new("/home/user/app")).unwrap();
    t.symlink("home/user/app", "nowhere");
    assert!(restore_from_trash(&env, &got).is_err());
}

#[test]
fn percent_encoding_keeps_unreserved() {
    assert_eq!(
        encode_path(Path::new("/a b/ü/x~1._-%")),
        "/a%20b/%C3%BC/x~1._-%25"
    );
}

#[test]
fn trash_dir_other_device_selection() {
    let devs: HashMap<&str, u64> = [
        ("/", 1),
        ("/mnt", 1),
        ("/mnt/usb", 2),
        ("/mnt/usb/data", 2),
        ("/mnt/usb/data/app", 2),
    ]
    .into();
    let dev_of = |p: &Path| devs.get(p.to_str().unwrap()).copied();
    assert_eq!(
        topdir_for(Path::new("/mnt/usb/data/app"), Path::new("/"), dev_of),
        Some(PathBuf::from("/mnt/usb"))
    );
    // Everything on one device walks up to `/`.
    let one = |_: &Path| Some(7);
    assert_eq!(
        topdir_for(Path::new("/a/b"), Path::new("/"), one),
        Some(PathBuf::from("/"))
    );
    // Unknown device for the item itself.
    assert_eq!(topdir_for(Path::new("/x"), Path::new("/"), |_| None), None);
}

#[test]
fn topdir_clamped_at_root() {
    let one = |_: &Path| Some(7);
    // On one device the walk stops at the (fake) root, not at `/`.
    assert_eq!(
        topdir_for(Path::new("/tmp/fx/mnt/a/b"), Path::new("/tmp/fx"), one),
        Some(PathBuf::from("/tmp/fx"))
    );
    // A path outside the root has no topdir.
    assert_eq!(
        topdir_for(Path::new("/etc/x"), Path::new("/tmp/fx"), one),
        None
    );
}

#[test]
fn trash_dir_same_device_uses_data_home() {
    let t = FixtureTree::new();
    t.write("home/user/a", b"x");
    let env = t.env();
    let d = trash_dir_for(&env, Path::new("/home/user/a")).unwrap();
    assert_eq!(d, env.data_home.join("Trash"));
}

#[test]
fn restore_refuses_reused_trash_name() {
    let t = FixtureTree::new();
    t.write("home/user/a/config/x", b"A");
    t.write("home/user/b/config/x", b"B");
    let env = t.env();
    let a = move_to_trash(&env, Path::new("/home/user/a/config")).unwrap();
    fs::remove_dir_all(env.sys_path(&a.files_path)).unwrap();
    fs::remove_file(env.sys_path(&a.info_path)).unwrap();
    let b = move_to_trash(&env, Path::new("/home/user/b/config")).unwrap();
    assert_eq!(a.files_path, b.files_path);
    match restore_from_trash(&env, &a) {
        Err(CoreError::Io { source, .. }) => {
            assert_eq!(source.kind(), io::ErrorKind::InvalidData)
        }
        other => panic!("expected InvalidData, got {other:?}"),
    }
    assert_eq!(
        fs::read(env.sys_path(&b.files_path).join("x")).unwrap(),
        b"B"
    );
    assert!(env.sys_path(&b.info_path).exists());
}

#[test]
fn restore_never_replaces_empty_dir_that_appears() {
    let t = FixtureTree::new();
    t.write("home/user/app/x", b"d");
    let env = t.env();
    let got = move_to_trash(&env, Path::new("/home/user/app")).unwrap();
    // Simulates losing the check-then-rename race: the destination exists
    // as an empty dir at rename time.
    let dest = env.sys_path(Path::new("/home/user/app"));
    fs::create_dir(&dest).unwrap();
    let err = rename_noreplace(&env.sys_path(&got.files_path), &dest).unwrap_err();
    assert_eq!(err.kind(), io::ErrorKind::AlreadyExists);
    assert!(env.sys_path(&got.files_path).join("x").exists());
    assert!(restore_from_trash(&env, &got).is_err());
    assert!(env.sys_path(&got.files_path).exists());
}

#[test]
fn trash_refuses_symlinked_trash_dir() {
    use std::os::unix::fs::PermissionsExt;
    let t = FixtureTree::new();
    t.write("home/user/a", b"x");
    let target = t.mkdir("elsewhere");
    fs::set_permissions(&target, fs::Permissions::from_mode(0o700)).unwrap();
    let env = t.env();
    fs::create_dir_all(&env.data_home).unwrap();
    std::os::unix::fs::symlink(&target, env.data_home.join("Trash")).unwrap();
    let err = move_to_trash(&env, Path::new("/home/user/a")).unwrap_err();
    assert!(err.to_string().contains("not a real directory"), "{err}");
    assert!(real(&env, "/home/user/a").exists());
}

#[test]
fn trash_refuses_trash_outside_home() {
    let t = FixtureTree::new();
    t.write("home/user/a", b"x");
    t.mkdir("data");
    let mut env = t.env();
    env.data_home = t.dir.path().join("data");
    let err = move_to_trash(&env, Path::new("/home/user/a")).unwrap_err();
    assert!(err.to_string().contains("outside your home"), "{err}");
    assert!(real(&env, "/home/user/a").exists());
    assert!(!env.data_home.join("Trash").exists());
}

#[test]
fn trash_under_home_still_works() {
    let t = FixtureTree::new();
    t.write("home/user/a", b"x");
    let env = t.env();
    let got = move_to_trash(&env, Path::new("/home/user/a")).unwrap();
    assert!(env.sys_path(&got.files_path).exists());
    assert!(!real(&env, "/home/user/a").exists());
}

#[test]
fn trash_works_with_existing_0755_dirs() {
    use std::os::unix::fs::PermissionsExt;
    let t = FixtureTree::new();
    t.write("home/user/a", b"x");
    let env = t.env();
    let trash = env.data_home.join("Trash");
    for d in [&trash, &trash.join("files"), &trash.join("info")] {
        fs::create_dir_all(d).unwrap();
        fs::set_permissions(d, fs::Permissions::from_mode(0o755)).unwrap();
    }
    let got = move_to_trash(&env, Path::new("/home/user/a")).unwrap();
    assert!(env.sys_path(&got.files_path).exists());
}

#[test]
fn trash_refuses_writable_by_others() {
    use std::os::unix::fs::PermissionsExt;
    for mode in [0o777, 0o775] {
        let t = FixtureTree::new();
        t.write("home/user/a", b"x");
        let env = t.env();
        let trash = env.data_home.join("Trash");
        fs::create_dir_all(&trash).unwrap();
        fs::set_permissions(&trash, fs::Permissions::from_mode(mode)).unwrap();
        assert!(move_to_trash(&env, Path::new("/home/user/a")).is_err());
        assert!(real(&env, "/home/user/a").exists());
    }
}

#[test]
fn non_utf8_name_round_trip() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;
    let t = FixtureTree::new();
    let env = t.env();
    let mut name = b"/home/user/caf".to_vec();
    name.push(0xE9);
    let p = PathBuf::from(OsString::from_vec(name));
    fs::create_dir_all(env.sys_path(&p)).unwrap();
    let got = move_to_trash(&env, &p).unwrap();
    let info = fs::read_to_string(env.sys_path(&got.info_path)).unwrap();
    assert!(info.contains("Path=/home/user/caf%E9\n"));
    restore_from_trash(&env, &got).unwrap();
    assert!(env.sys_path(&p).is_dir());
}

#[test]
fn topdir_trash_root_must_be_private_and_ours() {
    use std::os::unix::fs::PermissionsExt;
    let t = FixtureTree::new();
    let env = t.env();
    let root = t.dir.path().join(format!("mnt/.Trash-{}", env.uid));
    // Missing: created, mode 0700.
    ensure_topdir_trash(&env, &root).unwrap();
    assert_eq!(fs::metadata(&root).unwrap().mode() & 0o777, 0o700);
    // The same rule restore applies: anything but 0700 is refused.
    for mode in [0o755, 0o750, 0o500, 0o770, 0o701] {
        fs::set_permissions(&root, fs::Permissions::from_mode(mode)).unwrap();
        assert!(ensure_topdir_trash(&env, &root).is_err(), "{mode:o}");
    }
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
    assert!(ensure_topdir_trash(&env, &root).is_ok());
    // Not ours.
    let mut other = env.clone();
    other.uid = env.uid.wrapping_add(1);
    assert!(ensure_topdir_trash(&other, &root).is_err());
    // A symlink to a private directory.
    let link = t.dir.path().join("mnt/link");
    std::os::unix::fs::symlink(&root, &link).unwrap();
    assert!(ensure_topdir_trash(&env, &link).is_err());
}
