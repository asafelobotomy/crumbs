//! Sequential disk-usage measurement (allocated size, `st_blocks * 512`).

use std::collections::HashSet;
use std::fs;
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
