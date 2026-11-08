//! Sequential disk-usage measurement (allocated size, `st_blocks * 512`).

use std::collections::HashSet;
use std::fs;
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::env::Env;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct DiskUsage {
    pub bytes: u64,
    pub files: u64,
    pub errors: u64,
}

/// Measures the allocated size of `in_system` (a file or a tree).
///
/// Does not follow symlinks, does not cross devices, counts each
/// `(dev, ino)` once, and counts unreadable entries in `errors`.
pub fn disk_usage(env: &Env, in_system: &Path) -> DiskUsage {
    let start = env.sys(&in_system.to_string_lossy());
    let mut usage = DiskUsage::default();
    let Ok(meta) = fs::symlink_metadata(&start) else {
        usage.errors = 1;
        return usage;
    };
    let dev = meta.dev();
    let mut seen: HashSet<(u64, u64)> = HashSet::new();
    let mut stack: Vec<PathBuf> = Vec::new();
    visit(&start, &meta, dev, &mut seen, &mut stack, &mut usage);
    while let Some(dir) = stack.pop() {
        let entries = match fs::read_dir(&dir) {
            Ok(e) => e,
            Err(_) => {
                usage.errors += 1;
                continue;
            }
        };
        for entry in entries {
            let Ok(entry) = entry else {
                usage.errors += 1;
                continue;
            };
            let path = entry.path();
            match fs::symlink_metadata(&path) {
                Ok(m) => visit(&path, &m, dev, &mut seen, &mut stack, &mut usage),
                Err(_) => usage.errors += 1,
            }
        }
    }
    usage
}

/// Whether any entry in the tree at the real path `real` lies on another
/// mount than `real` itself (a mount point inside it, including a bind
/// mount of the same filesystem). Does not follow symlinks and stops at the
/// first such entry. `mount_of` gives an entry's mount ([`mount_id`]
/// outside tests); `None` from it, like any read error, is returned as an
/// error, so callers can fail closed.
pub(crate) fn crosses_mount(
    real: &Path,
    mount_of: &dyn Fn(&Path, &fs::Metadata) -> Option<u64>,
) -> io::Result<bool> {
    let unknown =
        |p: &Path| io::Error::other(format!("cannot determine the mount of {}", p.display()));
    let meta = fs::symlink_metadata(real)?;
    let root = mount_of(real, &meta).ok_or_else(|| unknown(real))?;
    if !meta.is_dir() {
        return Ok(false);
    }
    let mut stack = vec![real.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir)? {
            let path = entry?.path();
            let m = fs::symlink_metadata(&path)?;
            if mount_of(&path, &m).ok_or_else(|| unknown(&path))? != root {
                return Ok(true);
            }
            if m.is_dir() {
                stack.push(path);
            }
        }
    }
    Ok(false)
}

/// The mount ID of the entry at the real path `path` (not following a
/// final symlink), from `statx`. Where the kernel doesn't report mount IDs,
/// falls back to the device in `meta` (its `lstat`). `None` when `statx`
/// fails.
pub(crate) fn mount_id(path: &Path, meta: &fs::Metadata) -> Option<u64> {
    use rustix::fs::{AtFlags, CWD, StatxFlags, statx};
    use rustix::io::Errno;
    match statx(CWD, path, AtFlags::SYMLINK_NOFOLLOW, StatxFlags::MNT_ID) {
        Ok(stx) if StatxFlags::from_bits_retain(stx.stx_mask).contains(StatxFlags::MNT_ID) => {
            Some(stx.stx_mnt_id)
        }
        Ok(_) | Err(Errno::NOSYS) => Some(meta.dev()),
        Err(_) => None,
    }
}

/// Accounts for one entry; queues it when it is a directory.
fn visit(
    path: &Path,
    meta: &fs::Metadata,
    dev: u64,
    seen: &mut HashSet<(u64, u64)>,
    stack: &mut Vec<PathBuf>,
    usage: &mut DiskUsage,
) {
    if meta.dev() != dev || !seen.insert((meta.dev(), meta.ino())) {
        return;
    }
    usage.bytes = usage
        .bytes
        .saturating_add(meta.blocks().saturating_mul(512));
    if meta.is_dir() {
        stack.push(path.to_path_buf());
    } else {
        usage.files += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::FixtureTree;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn sums_files_in_tree() {
        let t = FixtureTree::new();
        t.write("/data/a", &[1u8; 5000]);
        t.write("/data/sub/b", &[2u8; 6000]);
        t.write("/data/sub/deep/c", &[3u8; 7000]);
        let u = disk_usage(&t.env(), Path::new("/data"));
        assert!(u.bytes >= 18000, "bytes = {}", u.bytes);
        assert_eq!(u.files, 3);
        assert_eq!(u.errors, 0);
    }

    #[test]
    fn hardlink_counted_once() {
        let t = FixtureTree::new();
        let a = t.write("/data/a", &[1u8; 100_000]);
        let b = t.dir.path().join("data/b");
        fs::hard_link(&a, &b).unwrap();
        let both = disk_usage(&t.env(), Path::new("/data"));
        let one = disk_usage(&t.env(), Path::new("/data/a"));
        assert_eq!(both.files, 1);
        assert!(both.bytes >= 100_000);
        // Only the directory's own blocks differ from the single file.
        assert!(both.bytes - one.bytes < 100_000, "{both:?} vs {one:?}");
    }

    #[test]
    fn symlink_not_followed() {
        let t = FixtureTree::new();
        t.write("/big/file", &[1u8; 1_000_000]);
        t.symlink("/data/link", "/big/file");
        let u = disk_usage(&t.env(), Path::new("/data"));
        assert!(u.bytes < 1_000_000, "bytes = {}", u.bytes);
        assert_eq!(u.files, 1);
    }

    #[test]
    fn unreadable_dir_counts_error() {
        if rustix::process::geteuid().is_root() {
            return;
        }
        let t = FixtureTree::new();
        t.write("/data/ok", &[1u8; 5000]);
        let locked = t.mkdir("/data/locked");
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
        let u = disk_usage(&t.env(), Path::new("/data"));
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(u.errors, 1);
        assert_eq!(u.files, 1);
        assert!(u.bytes >= 5000);
    }

    #[test]
    fn crosses_mount_finds_nested_mount() {
        let t = FixtureTree::new();
        t.write("/data/a/b/deep/file", b"x");
        t.symlink("/data/link", "/elsewhere");
        let root = t.dir.path().join("data");
        let same = |_: &Path, m: &fs::Metadata| Some(m.dev());
        assert!(!crosses_mount(&root, &same).unwrap());
        let deep = root.join("a/b/deep");
        let other = |p: &Path, m: &fs::Metadata| {
            Some(if p.starts_with(&deep) {
                m.dev() + 1
            } else {
                m.dev()
            })
        };
        assert!(crosses_mount(&root, &other).unwrap());
        assert!(crosses_mount(&root.join("nope"), &same).is_err());
        // The real lookup sees one mount in a plain tree.
        assert!(!crosses_mount(&root, &mount_id).unwrap());
    }

    #[test]
    fn crosses_mount_detects_same_dev_bind_mount() {
        let t = FixtureTree::new();
        t.write("/data/a/bind/file", b"x");
        let root = t.dir.path().join("data");
        let bind = root.join("a/bind");
        // A bind mount of the same filesystem: same `st_dev`, another mount ID.
        let mnt = |p: &Path, _: &fs::Metadata| Some(if p.starts_with(&bind) { 7 } else { 3 });
        let dev = |p: &Path, m: &fs::Metadata| {
            assert_eq!(m.dev(), fs::symlink_metadata(&root).unwrap().dev(), "{p:?}");
            Some(m.dev())
        };
        assert!(!crosses_mount(&root, &dev).unwrap());
        assert!(crosses_mount(&root, &mnt).unwrap());
        // An entry whose mount can't be determined fails closed.
        let unknown = |p: &Path, _: &fs::Metadata| (!p.starts_with(&bind)).then_some(3);
        assert!(crosses_mount(&root, &unknown).is_err());
    }

    #[test]
    fn missing_path_is_zero_with_error() {
        let t = FixtureTree::new();
        let u = disk_usage(&t.env(), Path::new("/nope"));
        assert_eq!(
            u,
            DiskUsage {
                bytes: 0,
                files: 0,
                errors: 1
            }
        );
    }
}
