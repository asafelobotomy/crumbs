//! Small no-follow file helpers shared by the executor and restore.

use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

/// `path` with `suffix` appended to its final component.
pub(crate) fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut s = OsString::from(path.as_os_str());
    s.push(suffix);
    PathBuf::from(s)
}

/// Opens a regular file for reading without following a final symlink.
/// Anything other than a regular file is refused.
pub(crate) fn open_regular_nofollow(real: &Path) -> io::Result<File> {
    use rustix::fs::{Mode, OFlags};
    let fd = rustix::fs::open(
        real,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK,
        Mode::empty(),
    )?;
    let file = File::from(fd);
    if !file.metadata()?.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "not a regular file",
        ));
    }
    Ok(file)
}

pub(crate) fn read_all(mut file: &File) -> io::Result<Vec<u8>> {
    let mut buf = Vec::new();
    file.read_to_end(&mut buf)?;
    Ok(buf)
}

/// Creates `real` (never overwriting anything, links included) with exactly
/// `mode`, writes `bytes` and syncs. A partial file is removed on failure.
pub(crate) fn write_new(real: &Path, bytes: &[u8], mode: u32) -> io::Result<()> {
    let mut f = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(real)?;
    let res = f
        .set_permissions(fs::Permissions::from_mode(mode))
        .and_then(|()| f.write_all(bytes))
        .and_then(|()| f.sync_all());
    if res.is_err() {
        let _ = fs::remove_file(real);
    }
    res
}

/// Replaces `real` atomically: writes `<real><tmp_suffix>` (create_new) and
/// renames it over `real`. The rename replaces the directory entry itself and
/// never follows a symlink at `real`.
pub(crate) fn replace_via_temp(
    real: &Path,
    bytes: &[u8],
    mode: u32,
    tmp_suffix: &str,
) -> io::Result<()> {
    let tmp = with_suffix(real, tmp_suffix);
    write_new(&tmp, bytes, mode)?;
    fs::rename(&tmp, real).inspect_err(|_| {
        let _ = fs::remove_file(&tmp);
    })
}

/// Drops every line exactly equal to one of `remove` (compared like
/// `str::lines`: without the `\n` or `\r\n` terminator). Empty entries in
/// `remove` are ignored, so blank lines always survive. Other bytes are kept
/// unchanged.
pub(crate) fn filter_lines(content: &[u8], remove: &[String]) -> Vec<u8> {
    let remove: Vec<&[u8]> = remove
        .iter()
        .map(|r| r.as_bytes())
        .filter(|r| !r.is_empty())
        .collect();
    let mut out = Vec::with_capacity(content.len());
    for chunk in content.split_inclusive(|&b| b == b'\n') {
        let line = match chunk.strip_suffix(b"\n") {
            Some(l) => l.strip_suffix(b"\r").unwrap_or(l),
            None => chunk,
        };
        if !remove.contains(&line) {
            out.extend_from_slice(chunk);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filter_exact_lines_only() {
        let rm = vec!["a=x".to_string(), String::new()];
        let got = filter_lines(b"a=x\n a=x\na=x;\n\na=x\r\nb\na=x", &rm);
        assert_eq!(got, b" a=x\na=x;\n\nb\n");
        assert_eq!(filter_lines(b"keep\n", &rm), b"keep\n");
    }
}
