//! Freedesktop trash (spec 1.0), implemented through `Env` so tests can use a
//! fake root. Only `$topdir/.Trash-$uid` is ever created, never `.Trash/$uid`.

use std::ffi::OsStr;
use std::fs::{self, DirBuilder, OpenOptions};
use std::io::{self, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::env::Env;
use crate::error::CoreError;
use crate::safety::is_canonical;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Trashed {
    pub original: PathBuf,
    pub files_path: PathBuf,
    pub info_path: PathBuf,
}

fn io_err(path: &Path, source: io::Error) -> CoreError {
    CoreError::Io {
        path: path.to_path_buf(),
        source,
    }
}

fn mkdir_private(path: &Path) -> Result<(), CoreError> {
    DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(path)
        .map_err(|e| io_err(path, e))
}

/// The trash dir must be a real directory (not a symlink) owned by us with no
/// group/other access.
pub(crate) fn validate_private_dir(env: &Env, path: &Path) -> Result<(), CoreError> {
    let m = fs::symlink_metadata(path).map_err(|e| io_err(path, e))?;
    let bad = |msg: &str| {
        io_err(
            path,
            io::Error::new(io::ErrorKind::PermissionDenied, msg.to_string()),
        )
    };
    if !m.file_type().is_dir() {
        return Err(bad("trash path is not a real directory"));
    }
    if m.uid() != env.uid {
        return Err(bad("trash directory is not owned by the current user"));
    }
    if m.mode() & 0o022 != 0 {
        return Err(bad("trash directory is writable by other users"));
    }
    Ok(())
}

/// Creates (if needed) and validates a trash directory.
fn ensure_private_dir(env: &Env, path: &Path) -> Result<(), CoreError> {
    mkdir_private(path)?;
    validate_private_dir(env, path)
}

/// Rename that never replaces an existing destination (RENAME_NOREPLACE),
/// falling back to plain rename only where the filesystem lacks support.
fn rename_noreplace(src: &Path, dst: &Path) -> io::Result<()> {
    use rustix::fs::{CWD, RenameFlags, renameat_with};
    use rustix::io::Errno;
    match renameat_with(CWD, src, CWD, dst, RenameFlags::NOREPLACE) {
        Ok(()) => Ok(()),
        Err(Errno::INVAL | Errno::NOSYS) => fs::rename(src, dst),
        Err(e) => Err(io::Error::from(e)),
    }
}

fn dev_of_real(p: &Path) -> Option<u64> {
    fs::symlink_metadata(p).ok().map(|m| m.dev())
}

/// Highest ancestor of `path` that is still on `path`'s device. `None` when
/// the device of `path` is unknown or no ancestor shares it.
pub(crate) fn topdir_for(path: &Path, dev_of: impl Fn(&Path) -> Option<u64>) -> Option<PathBuf> {
    let dev = dev_of(path)?;
    let mut top: Option<&Path> = None;
    for a in path.ancestors().skip(1) {
        if dev_of(a) == Some(dev) {
            top = Some(a);
        } else {
            break;
        }
    }
    top.map(Path::to_path_buf)
}

/// Nearest existing ancestor (or self) of `p`, for device lookup.
fn nearest_existing(p: &Path) -> Option<&Path> {
    p.ancestors().find(|a| fs::symlink_metadata(a).is_ok())
}

/// The trash directory (real path) for an item at `in_system`, created with
/// mode 0700 when missing.
pub fn trash_dir_for(env: &Env, in_system: &Path) -> Result<PathBuf, CoreError> {
    let item = env.sys_path(in_system);
    let home_trash = env.data_home.join("Trash");
    // A missing item cannot be moved anyway; let the rename report that.
    let Some(item_dev) = dev_of_real(&item) else {
        ensure_private_dir(env, &home_trash)?;
        return Ok(home_trash);
    };
    let data_dev = nearest_existing(&env.data_home).and_then(dev_of_real);
    if data_dev == Some(item_dev) {
        ensure_private_dir(env, &home_trash)?;
        return Ok(home_trash);
    }
    let fail = |msg: &str| io_err(in_system, io::Error::other(msg.to_string()));
    let top = topdir_for(&item, dev_of_real).ok_or_else(|| fail("no usable trash topdir"))?;
    if env.unsys(&top) == Path::new("/") {
        return Err(fail("refusing to use / as a trash topdir"));
    }
    let dir = top.join(format!(".Trash-{}", env.uid));
    ensure_private_dir(env, &dir)?;
    Ok(dir)
}

/// Percent-encodes an absolute path; keeps `A-Za-z0-9-._~/`.
fn encode_path(p: &Path) -> String {
    let mut out = String::new();
    for &b in p.as_os_str().as_bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~' | b'/') {
            out.push(char::from(b));
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn info_name(info: &Path, name: &OsStr) -> PathBuf {
    let mut f = name.to_os_string();
    f.push(".trashinfo");
    info.join(f)
}

fn name_taken(files: &Path, info: &Path, name: &OsStr) -> bool {
    fs::symlink_metadata(files.join(name)).is_ok()
        || fs::symlink_metadata(info_name(info, name)).is_ok()
}

/// Moves the item itself (never a symlink target) into the trash.
pub fn move_to_trash(env: &Env, in_system: &Path) -> Result<Trashed, CoreError> {
    if !is_canonical(in_system) {
        return Err(io_err(
            in_system,
            io::Error::new(io::ErrorKind::InvalidInput, "path is not canonical"),
        ));
    }
    let item = env.sys_path(in_system);
    let trash = trash_dir_for(env, in_system)?;
    let files = trash.join("files");
    let info = trash.join("info");
    ensure_private_dir(env, &files)?;
    ensure_private_dir(env, &info)?;

    let base = in_system
        .file_name()
        .ok_or_else(|| {
            io_err(
                in_system,
                io::Error::new(io::ErrorKind::InvalidInput, "path has no file name"),
            )
        })?
        .to_os_string();
    let content = format!(
        "[Trash Info]\nPath={}\nDeletionDate={}\n",
        encode_path(in_system),
        jiff::Zoned::now().strftime("%Y-%m-%dT%H:%M:%S")
    );

    let mut n = 1u32;
    let (name, info_path) = loop {
        let name = if n == 1 {
            base.clone()
        } else {
            let mut s = base.clone();
            s.push(format!(".{n}"));
            s
        };
        n += 1;
        if name_taken(&files, &info, &name) {
            continue;
        }
        let info_path = info_name(&info, &name);
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&info_path)
        {
            Ok(mut f) => {
                f.write_all(content.as_bytes())
                    .map_err(|e| io_err(&info_path, e))
                    .inspect_err(|_| {
                        let _ = fs::remove_file(&info_path);
                    })?;
                break (name, info_path);
            }
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(io_err(&info_path, e)),
        }
    };

    let files_path = files.join(&name);
    if let Err(e) = rename_noreplace(&item, &files_path) {
        let _ = fs::remove_file(&info_path);
        return Err(io_err(in_system, e));
    }
    Ok(Trashed {
        original: in_system.to_path_buf(),
        files_path: env.unsys(&files_path),
        info_path: env.unsys(&info_path),
    })
}

/// Moves a trashed item back. Refuses when the original path is occupied.
pub fn restore_from_trash(env: &Env, t: &Trashed) -> Result<(), CoreError> {
    let info = env.sys_path(&t.info_path);
    let recorded = fs::read_to_string(&info).map_err(|e| io_err(&t.info_path, e))?;
    let expected = encode_path(&t.original);
    let matches = recorded
        .lines()
        .any(|l| l.strip_prefix("Path=") == Some(expected.as_str()));
    if !matches {
        return Err(io_err(
            &t.info_path,
            io::Error::new(io::ErrorKind::InvalidData, "trash entry no longer matches"),
        ));
    }
    let dest = env.sys_path(&t.original);
    if fs::symlink_metadata(&dest).is_ok() {
        return Err(io_err(
            &t.original,
            io::Error::new(io::ErrorKind::AlreadyExists, "destination already exists"),
        ));
    }
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent).map_err(|e| io_err(&t.original, e))?;
    }
    let src = env.sys_path(&t.files_path);
    rename_noreplace(&src, &dest).map_err(|e| io_err(&t.files_path, e))?;
    fs::remove_file(&info).map_err(|e| io_err(&t.info_path, e))
}

#[cfg(test)]
mod tests {
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
            topdir_for(Path::new("/mnt/usb/data/app"), dev_of),
            Some(PathBuf::from("/mnt/usb"))
        );
        // Everything on one device walks up to `/`.
        let one = |_: &Path| Some(7);
        assert_eq!(topdir_for(Path::new("/a/b"), one), Some(PathBuf::from("/")));
        // Unknown device for the item itself.
        assert_eq!(topdir_for(Path::new("/x"), |_| None), None);
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
}
