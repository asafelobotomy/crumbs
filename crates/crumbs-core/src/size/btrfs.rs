//! The on-disk size of files on btrfs, read from the filesystem's extent
//! items the way `compsize` does. btrfs reports a compressed file's
//! `st_blocks` uncompressed; only `BTRFS_IOC_TREE_SEARCH`, which needs
//! root, tells how much space its extents really take.

use std::collections::HashSet;
use std::io;
use std::os::fd::OwnedFd;
use std::path::Path;

use rustix::fs::{CWD, Mode, OFlags, openat};
use rustix::ioctl::{Opcode, Updater, ioctl, opcode};

/// `BTRFS_EXTENT_DATA_KEY`: a file extent item.
const EXTENT_DATA_KEY: u32 = 108;
/// `BTRFS_FILE_EXTENT_INLINE`: the data sits in the item itself.
const EXTENT_INLINE: u8 = 0;
/// Offset of the extent type in `struct btrfs_file_extent_item`.
const TYPE_AT: usize = 20;
/// Where inline data, or `disk_bytenr` then `disk_num_bytes`, begins.
const DATA_AT: usize = 21;
/// `struct btrfs_ioctl_search_header`.
const HEADER_LEN: usize = 32;
const ARGS_LEN: usize = 4096;
const KEY_LEN: usize = 104;

/// `struct btrfs_ioctl_search_key`.
#[repr(C)]
#[derive(Default)]
struct SearchKey {
    tree_id: u64,
    min_objectid: u64,
    max_objectid: u64,
    min_offset: u64,
    max_offset: u64,
    min_transid: u64,
    max_transid: u64,
    min_type: u32,
    max_type: u32,
    nr_items: u32,
    unused: u32,
    unused1: u64,
    unused2: u64,
    unused3: u64,
    unused4: u64,
}

/// `struct btrfs_ioctl_search_args`.
#[repr(C)]
struct SearchArgs {
    key: SearchKey,
    buf: [u8; ARGS_LEN - KEY_LEN],
}

const _: () = assert!(size_of::<SearchKey>() == KEY_LEN);
const _: () = assert!(size_of::<SearchArgs>() == ARGS_LEN);

/// `BTRFS_IOC_TREE_SEARCH`: `_IOWR(0x94, 17, struct btrfs_ioctl_search_args)`.
const TREE_SEARCH: Opcode = opcode::read_write::<SearchArgs>(0x94, 17);

/// Sums the on-disk size of files in one subvolume, counting each extent
/// once however many files (or reflinks) share it.
pub(super) struct Extents {
    /// A directory in the subvolume; the search runs in its tree.
    dir: OwnedFd,
    seen: HashSet<u64>,
    pub bytes: u64,
}

impl Extents {
    /// Opens the real directory `dir` for searching. Fails unless running
    /// as root on btrfs (the first search says which).
    pub fn open(dir: &Path) -> io::Result<Self> {
        let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
        let dir = openat(CWD, dir, flags, Mode::empty())?;
        Ok(Self {
            dir,
            seen: HashSet::new(),
            bytes: 0,
        })
    }

    /// Adds the extents of inode `ino`, which must be in the directory's
    /// subvolume (the same `st_dev`).
    pub fn add_inode(&mut self, ino: u64) -> io::Result<()> {
        let mut args = Box::new(SearchArgs {
            key: SearchKey::default(),
            buf: [0; ARGS_LEN - KEY_LEN],
        });
        let mut min_offset = 0;
        loop {
            args.key = SearchKey {
                // 0: the tree of the subvolume `dir` is in.
                tree_id: 0,
                min_objectid: ino,
                max_objectid: ino,
                min_offset,
                max_offset: u64::MAX,
                min_transid: 0,
                max_transid: u64::MAX,
                min_type: EXTENT_DATA_KEY,
                max_type: EXTENT_DATA_KEY,
                nr_items: 4096,
                ..SearchKey::default()
            };
            // SAFETY: `TREE_SEARCH` is the kernel's opcode for
            // `struct btrfs_ioctl_search_args`, whose layout `SearchArgs`
            // repeats (its size is checked above); the kernel only reads
            // and writes inside it.
            unsafe {
                ioctl(
                    &self.dir,
                    Updater::<TREE_SEARCH, SearchArgs>::new(&mut args),
                )
            }?;
            let found = args.key.nr_items;
            if found == 0 {
                return Ok(());
            }
            let last = account(&args.buf, found, &mut self.seen, &mut self.bytes)
                .ok_or_else(|| io::Error::other("malformed btrfs search result"))?;
            match last.checked_add(1) {
                Some(next) => min_offset = next,
                None => return Ok(()),
            }
        }
    }
}

/// Adds the on-disk size of the `count` items in a search result `buf` to
/// `bytes`: an inline extent's stored data, and each regular or
/// preallocated extent's `disk_num_bytes` the first time its `disk_bytenr`
/// is seen (a hole, `disk_bytenr` 0, takes no space). Returns the key
/// offset of the last item, or `None` when the buffer is malformed.
fn account(buf: &[u8], count: u32, seen: &mut HashSet<u64>, bytes: &mut u64) -> Option<u64> {
    let u64_at = |b: &[u8], at: usize| -> Option<u64> {
        Some(u64::from_le_bytes(b.get(at..at + 8)?.try_into().ok()?))
    };
    let u32_at = |b: &[u8], at: usize| -> Option<u32> {
        Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
    };
    let mut pos = 0usize;
    let mut last = None;
    for _ in 0..count {
        let header = buf.get(pos..pos + HEADER_LEN)?;
        let offset = u64_at(header, 16)?;
        let kind = u32_at(header, 24)?;
        let len = usize::try_from(u32_at(header, 28)?).ok()?;
        let item = buf.get(pos + HEADER_LEN..pos + HEADER_LEN + len)?;
        pos += HEADER_LEN + len;
        last = Some(offset);
        if kind != EXTENT_DATA_KEY {
            continue;
        }
        if *item.get(TYPE_AT)? == EXTENT_INLINE {
            *bytes = bytes.saturating_add((len.saturating_sub(DATA_AT)) as u64);
            continue;
        }
        let disk_bytenr = u64_at(item, DATA_AT)?;
        let disk_num_bytes = u64_at(item, DATA_AT + 8)?;
        if disk_bytenr != 0 && seen.insert(disk_bytenr) {
            *bytes = bytes.saturating_add(disk_num_bytes);
        }
    }
    last
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header(offset: u64, kind: u32, len: usize) -> Vec<u8> {
        let mut h = Vec::new();
        h.extend_from_slice(&1u64.to_le_bytes()); // transid
        h.extend_from_slice(&257u64.to_le_bytes()); // objectid
        h.extend_from_slice(&offset.to_le_bytes());
        h.extend_from_slice(&kind.to_le_bytes());
        h.extend_from_slice(&(len as u32).to_le_bytes());
        h
    }

    fn regular(offset: u64, disk_bytenr: u64, disk_num_bytes: u64) -> Vec<u8> {
        let mut item = vec![0u8; 53];
        item[16] = 3; // compression: zstd
        item[TYPE_AT] = 1;
        item[21..29].copy_from_slice(&disk_bytenr.to_le_bytes());
        item[29..37].copy_from_slice(&disk_num_bytes.to_le_bytes());
        item[45..53].copy_from_slice(&131_072u64.to_le_bytes()); // num_bytes
        let mut out = header(offset, EXTENT_DATA_KEY, item.len());
        out.extend(item);
        out
    }

    fn inline(offset: u64, data: usize) -> Vec<u8> {
        let mut item = vec![0u8; DATA_AT + data];
        item[TYPE_AT] = EXTENT_INLINE;
        let mut out = header(offset, EXTENT_DATA_KEY, item.len());
        out.extend(item);
        out
    }

    #[test]
    fn counts_each_extent_once_and_inline_data() {
        let buf = [
            regular(0, 4096, 20_000),
            regular(131_072, 8192, 30_000),
            // Another reference to the first extent (a reflink).
            regular(262_144, 4096, 20_000),
            // A hole.
            regular(393_216, 0, 0),
            inline(524_288, 300),
        ]
        .concat();
        let mut seen = HashSet::new();
        let mut bytes = 0;
        assert_eq!(account(&buf, 5, &mut seen, &mut bytes), Some(524_288));
        assert_eq!(bytes, 50_300);
        // The extents are shared across files of one walk too.
        let again = regular(0, 8192, 30_000);
        assert_eq!(account(&again, 1, &mut seen, &mut bytes), Some(0));
        assert_eq!(bytes, 50_300);
    }

    #[test]
    fn other_item_types_are_skipped() {
        let mut buf = header(0, 1, 4);
        buf.extend([0u8; 4]);
        buf.extend(regular(0, 4096, 100));
        let mut bytes = 0;
        assert_eq!(account(&buf, 2, &mut HashSet::new(), &mut bytes), Some(0));
        assert_eq!(bytes, 100);
    }

    #[test]
    fn malformed_results_are_refused() {
        let full = regular(0, 4096, 100);
        let mut bytes = 0;
        // Fewer bytes than the header says.
        assert_eq!(
            account(&full[..40], 1, &mut HashSet::new(), &mut bytes),
            None
        );
        // More items than the buffer holds.
        assert_eq!(account(&full, 2, &mut HashSet::new(), &mut bytes), None);
        // A regular extent item too short for its sizes.
        let mut short = header(0, EXTENT_DATA_KEY, 22);
        short.extend([
            0u8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0,
        ]);
        assert_eq!(account(&short, 1, &mut HashSet::new(), &mut bytes), None);
    }
}
