//! Sequential disk-usage measurement (allocated size, `st_blocks * 512`).

mod btrfs;

use std::collections::HashSet;
use std::fs;
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use rayon::prelude::*;
use serde::Serialize;

use crate::env::Env;
use crate::mounts::{MountFs, mount_fs};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct DiskUsage {
    pub bytes: u64,
    pub files: u64,
    pub errors: u64,
    /// `bytes` counts files that btrfs may keep compressed at their
    /// uncompressed size, so removing them can free less. False when there
    /// are none, or when crumbs read their real size (as root).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub uncompressed: bool,
}

/// Formats a byte count in IEC units with one decimal place.
pub fn fmt_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["KiB", "MiB", "GiB", "TiB", "PiB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64 / 1024.0;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    format!("{value:.1} {}", UNITS[unit])
}

/// Measures the allocated size of `in_system` (a file or a tree).
///
/// Does not follow symlinks, does not cross devices, counts each
/// `(dev, ino)` once, and counts unreadable entries in `errors`.
///
/// On btrfs, which reports a compressed file's blocks uncompressed, the
/// result says so in `uncompressed` (the mount compresses, or a file is
/// marked compressed). Running as root, crumbs instead reads the real size
/// of each file's extents, as `compsize` does, counting an extent shared by
/// several files of the tree once.
pub fn disk_usage(env: &Env, in_system: &Path) -> DiskUsage {
    let start = env.sys(&in_system.to_string_lossy());
    let Ok(meta) = fs::symlink_metadata(&start) else {
        return DiskUsage {
            errors: 1,
            ..DiskUsage::default()
        };
    };
    let mount = mount_fs(env, in_system);
    let btrfs = mount.as_ref().map(|m| m.fstype == "btrfs") == Some(true);
    if btrfs
        && rustix::process::geteuid().is_root()
        && let Some(usage) = exact_usage(&start, &meta)
    {
        return usage;
    }
    let compresses = btrfs.then(|| mount.as_ref().is_some_and(MountFs::btrfs_compresses));
    let mut walk = Walk::new(&meta, compresses, None);
    // Without an extent search, a walk cannot fail.
    let _ = walk.run(&start, &meta);
    walk.usage
}

/// [`disk_usage`] as root on btrfs: the real size of the extents. `None`
/// when a search fails (not root after all, or not btrfs), so the caller
/// falls back to counting blocks.
fn exact_usage(start: &Path, meta: &fs::Metadata) -> Option<DiskUsage> {
    let dir = if meta.is_dir() {
        start.to_path_buf()
    } else {
        let parent = start.parent()?;
        // The search runs in the parent's subvolume.
        if fs::symlink_metadata(parent).ok()?.dev() != meta.dev() {
            return None;
        }
        parent.to_path_buf()
    };
    let extents = btrfs::Extents::open(&dir).ok()?;
    let mut walk = Walk::new(meta, None, Some(extents));
    walk.run(start, meta).ok()?;
    let extents = walk.extents.take()?;
    walk.usage.bytes = walk.usage.bytes.saturating_add(extents.bytes);
    Some(walk.usage)
}

/// One measurement of a tree.
struct Walk {
    dev: u64,
    seen: HashSet<(u64, u64)>,
    stack: Vec<PathBuf>,
    usage: DiskUsage,
    /// On btrfs: whether the mount compresses. `None` elsewhere.
    compresses: Option<bool>,
    /// As root on btrfs: where a regular file's size is read from.
    extents: Option<btrfs::Extents>,
}

impl Walk {
    fn new(
        start: &fs::Metadata,
        compresses: Option<bool>,
        extents: Option<btrfs::Extents>,
    ) -> Self {
        Walk {
            dev: start.dev(),
            seen: HashSet::new(),
            stack: Vec::new(),
            usage: DiskUsage::default(),
            compresses,
            extents,
        }
    }

    /// Walks from `start`; fails only when an extent search fails.
    fn run(&mut self, start: &Path, meta: &fs::Metadata) -> io::Result<()> {
        self.visit(start, meta)?;
        while let Some(dir) = self.stack.pop() {
            let entries = match fs::read_dir(&dir) {
                Ok(e) => e,
                Err(_) => {
                    self.usage.errors += 1;
                    continue;
                }
            };
            for entry in entries {
                let Ok(entry) = entry else {
                    self.usage.errors += 1;
                    continue;
                };
                let path = entry.path();
                match fs::symlink_metadata(&path) {
                    Ok(m) => self.visit(&path, &m)?,
                    Err(_) => self.usage.errors += 1,
                }
            }
        }
        Ok(())
    }

    /// Accounts for one entry; queues it when it is a directory.
    fn visit(&mut self, path: &Path, meta: &fs::Metadata) -> io::Result<()> {
        if meta.dev() != self.dev || !self.seen.insert((meta.dev(), meta.ino())) {
            return Ok(());
        }
        let blocks = meta.blocks().saturating_mul(512);
        if meta.is_dir() {
            self.stack.push(path.to_path_buf());
            self.usage.bytes = self.usage.bytes.saturating_add(blocks);
            return Ok(());
        }
        self.usage.files += 1;
        if meta.is_file() && blocks > 0 {
            if let Some(extents) = &mut self.extents {
                return extents.add_inode(meta.ino());
            }
            if let Some(compresses) = self.compresses
                && !self.usage.uncompressed
                && (compresses || marked_compressed(path))
            {
                self.usage.uncompressed = true;
            }
        }
        self.usage.bytes = self.usage.bytes.saturating_add(blocks);
        Ok(())
    }
}

/// Whether the file at the real path `path` carries the compressed
/// attribute (`chattr +c`, or a btrfs `compression` property inherited from
/// its directory).
fn marked_compressed(path: &Path) -> bool {
    use rustix::fs::{AtFlags, CWD, StatxAttributes, StatxFlags, statx};
    let bit = StatxAttributes::COMPRESSED;
    statx(CWD, path, AtFlags::SYMLINK_NOFOLLOW, StatxFlags::empty())
        .is_ok_and(|s| s.stx_attributes_mask.contains(bit) && s.stx_attributes.contains(bit))
}

/// [`disk_usage`] for each of `paths` in parallel, in the same order. Each
/// walk is independent, so hard links are counted once per path.
pub fn disk_usage_many(env: &Env, paths: &[PathBuf]) -> Vec<DiskUsage> {
    paths.par_iter().map(|p| disk_usage(env, p)).collect()
}

/// Whether the tree at the real path `real` spans more than one mount:
/// `real` is itself a mount root (its mount differs from its parent's), or
/// an entry below it lies on another mount (including a bind mount of the
/// same filesystem). Does not follow symlinks and stops at the
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
    // The target may itself be a mount root (a FUSE mount in `/tmp`, say):
    // then it differs from its parent's mount.
    let parent = real
        .parent()
        .ok_or_else(|| io::Error::other(format!("{} has no parent", real.display())))?;
    let parent_meta = fs::symlink_metadata(parent)?;
    if mount_of(parent, &parent_meta).ok_or_else(|| unknown(parent))? != root {
        return Ok(true);
    }
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
    fn crosses_mount_detects_target_that_is_a_mount_root() {
        let t = FixtureTree::new();
        t.write("/tmp/mnt/inner", b"on another fs");
        t.write("/tmp/file", b"x");
        let mnt = t.dir.path().join("tmp/mnt");
        // The target itself is the mount root: it and everything below it
        // share one ID, which differs from its parent's.
        let mounted = |p: &Path, _: &fs::Metadata| Some(if p.starts_with(&mnt) { 9 } else { 1 });
        assert!(crosses_mount(&mnt, &mounted).unwrap());
        // A regular file whose parent is on the same mount.
        let file = t.dir.path().join("tmp/file");
        assert!(!crosses_mount(&file, &mounted).unwrap());
        // The parent's mount can't be determined: fail closed.
        let parent = t.dir.path().join("tmp");
        let no_parent = |p: &Path, _: &fs::Metadata| (p != parent).then_some(1);
        assert!(crosses_mount(&file, &no_parent).is_err());
        assert!(crosses_mount(&mnt, &no_parent).is_err());
    }

    #[test]
    fn disk_usage_many_matches_sequential() {
        let t = FixtureTree::new();
        t.write("/data/a/f1", &[1u8; 9000]);
        t.write("/data/a/sub/f2", &[1u8; 20_000]);
        t.write("/data/b/f3", &[1u8; 4000]);
        t.write("/data/c", &[1u8; 70_000]);
        let env = t.env();
        let paths: Vec<PathBuf> = ["/data/c", "/data/a", "/nope", "/data/b"]
            .iter()
            .map(PathBuf::from)
            .collect();
        let many = disk_usage_many(&env, &paths);
        let seq: Vec<DiskUsage> = paths.iter().map(|p| disk_usage(&env, p)).collect();
        assert_eq!(many, seq);
        assert_eq!(many[2].errors, 1);
        assert!(disk_usage_many(&env, &[]).is_empty());
    }

    fn mount_line(mount: &Path, opts: &str) -> String {
        format!("2 1 0:2 / {} rw - btrfs /dev/b {opts}\n", mount.display())
    }

    #[test]
    fn btrfs_compression_is_flagged() {
        if rustix::process::geteuid().is_root() {
            // As root on btrfs the size would be read exactly instead.
            return;
        }
        let t = FixtureTree::new();
        t.write("/z/a", &[1u8; 5000]);
        t.write("/z/empty", b"");
        t.write("/plain/b", &[1u8; 5000]);
        t.write("/ext/c", &[1u8; 5000]);
        let root = t.dir.path();
        let info = [
            "1 0 0:1 / / rw - ext4 /dev/a rw\n".to_string(),
            mount_line(&root.join("z"), "rw,compress=zstd:3"),
            mount_line(&root.join("plain"), "rw,ssd"),
        ]
        .concat();
        t.write("proc/self/mountinfo", info.as_bytes());
        let env = t.env();
        let z = disk_usage(&env, Path::new("/z"));
        assert!(z.uncompressed && z.bytes >= 5000, "{z:?}");
        // An empty file takes no blocks, so nothing is compressed.
        assert!(!disk_usage(&env, Path::new("/z/empty")).uncompressed);
        // btrfs without the mount option: only files marked compressed
        // count, and a fixture file is not.
        assert!(!disk_usage(&env, Path::new("/plain")).uncompressed);
        assert!(!disk_usage(&env, Path::new("/ext")).uncompressed);
        let json = serde_json::to_value(z).unwrap();
        assert_eq!(json["uncompressed"], true);
        let json = serde_json::to_value(disk_usage(&env, Path::new("/ext"))).unwrap();
        assert!(json.get("uncompressed").is_none());
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
                errors: 1,
                uncompressed: false,
            }
        );
    }
}
