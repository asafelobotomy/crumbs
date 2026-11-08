//! `trash`: emptying the home trash, one item per trashed entry.

use std::collections::BTreeSet;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::Read;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use super::{BloatItem, Category, ItemAction, ScanContext, Scanner};
use crate::env::display_path;
use crate::error::CoreError;
use crate::safety::{Fingerprint, Scope, fingerprint, is_allowed_target};
use crate::trash::validate_private_dir;

const INFO_SUFFIX: &[u8] = b".trashinfo";

/// Largest `.trashinfo` read for its label.
const INFO_READ_CAP: u64 = 64 * 1024;

fn io_err(path: &Path, source: std::io::Error) -> CoreError {
    CoreError::Io {
        path: path.to_path_buf(),
        source,
    }
}

/// Decodes `%XX` escapes; a malformed escape is kept as written.
fn percent_decode(s: &str) -> Vec<u8> {
    let b = s.as_bytes();
    let hex = |c: u8| char::from(c).to_digit(16);
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && let (Some(&h), Some(&l)) = (b.get(i + 1), b.get(i + 2))
            && let (Some(h), Some(l)) = (hex(h), hex(l))
        {
            out.push((h * 16 + l) as u8);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    out
}

/// The original path recorded in a `.trashinfo` (real path), read without
/// following a symlink.
fn original_path(info_real: &Path) -> Option<PathBuf> {
    let f = fs::OpenOptions::new()
        .read(true)
        .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32)
        .open(info_real)
        .ok()?;
    if !f.metadata().ok()?.is_file() {
        return None;
    }
    let mut buf = Vec::new();
    f.take(INFO_READ_CAP).read_to_end(&mut buf).ok()?;
    let text = String::from_utf8_lossy(&buf);
    let value = text.lines().find_map(|l| l.strip_prefix("Path="))?;
    Some(PathBuf::from(OsString::from_vec(percent_decode(value))))
}

/// `(in-system path, fingerprint)` when the path exists, is an allowed
/// home-trash target and can be fingerprinted. `Err(())` when it exists
/// but fails a check.
fn checked(cx: &ScanContext, real: &Path) -> Result<Option<(PathBuf, Fingerprint)>, ()> {
    if fs::symlink_metadata(real).is_err() {
        return Ok(None);
    }
    let path = cx.env.unsys(real);
    if !is_allowed_target(cx.env, cx.cfg, Scope::HomeTrash, &path) {
        return Err(());
    }
    let fp = fingerprint(cx.env, &path).map_err(|_| ())?;
    Ok(Some((path, fp)))
}

fn info_file(info: &Path, name: &OsStr) -> PathBuf {
    let mut f = name.to_os_string();
    f.push(OsStr::from_bytes(INFO_SUFFIX));
    info.join(f)
}

pub struct TrashScanner;

impl Scanner for TrashScanner {
    fn category(&self) -> Category {
        Category::Trash
    }

    fn scan(&self, cx: &ScanContext) -> Result<Vec<BloatItem>, CoreError> {
        let trash = cx.env.data_home.join("Trash");
        if matches!(fs::symlink_metadata(&trash), Err(e) if e.kind() == std::io::ErrorKind::NotFound)
        {
            return Ok(Vec::new());
        }
        let files = trash.join("files");
        let info = trash.join("info");
        for dir in [&trash, &files, &info] {
            validate_private_dir(cx.env, dir)?;
        }

        let mut names: BTreeSet<OsString> = BTreeSet::new();
        for entry in fs::read_dir(&files).map_err(|e| io_err(&files, e))? {
            names.insert(entry.map_err(|e| io_err(&files, e))?.file_name());
        }
        for entry in fs::read_dir(&info).map_err(|e| io_err(&info, e))? {
            let name = entry.map_err(|e| io_err(&info, e))?.file_name();
            if let Some(stem) = name.as_bytes().strip_suffix(INFO_SUFFIX)
                && !stem.is_empty()
            {
                names.insert(OsStr::from_bytes(stem).to_os_string());
            }
        }

        let mut out = Vec::new();
        for name in names {
            let file_real = files.join(&name);
            let info_real = info_file(&info, &name);
            let (Ok(file), Ok(meta)) = (checked(cx, &file_real), checked(cx, &info_real)) else {
                continue;
            };
            let info_only = file.is_none();
            let (path, fp, extra_paths) = match (file, meta) {
                (Some((p, fp)), Some(extra)) => (p, fp, vec![extra]),
                (Some((p, fp)), None) | (None, Some((p, fp))) => (p, fp, Vec::new()),
                (None, None) => continue,
            };
            let original = original_path(&info_real).map(|orig| {
                if orig.is_absolute() {
                    display_path(cx.env, &orig)
                } else {
                    orig.to_string_lossy().into_owned()
                }
            });
            let label = match (original, info_only) {
                (Some(orig), false) => orig,
                (Some(orig), true) => format!("{orig} (trash info only)"),
                (None, true) => format!("{} (trash info only)", name.to_string_lossy()),
                (None, false) => format!("{} (no trash info)", name.to_string_lossy()),
            };
            out.push(BloatItem {
                category: Category::Trash,
                path: Some(path),
                label,
                size: None,
                rebuildable: false,
                // Emptying the trash cannot be undone: opt-in, by naming
                // the category or ticking it with `-i`.
                preselected: false,
                note: None,
                action: ItemAction::Delete,
                scope: Scope::HomeTrash,
                fingerprint: Some(fp),
                extra_paths,
                app_id: None,
            });
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::percent_decode;

    #[test]
    fn percent_decode_cases() {
        assert_eq!(percent_decode("/a%20b"), b"/a b");
        assert_eq!(percent_decode("/caf%E9"), b"/caf\xe9");
        assert_eq!(percent_decode("/x%2"), b"/x%2");
        assert_eq!(percent_decode("/x%zz%41"), b"/x%zzA");
    }
}
