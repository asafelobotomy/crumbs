//! Mount points below a path, read from the kernel's mountinfo. A privileged
//! `rm -rf --one-file-system` skips a nested mount instead of failing, which
//! would leave a half-deleted tree, so callers refuse to start when any
//! mount lies at or below the target.

use std::ffi::OsString;
use std::fs;
use std::os::unix::ffi::OsStringExt;
use std::path::{Path, PathBuf};

use crate::env::Env;
use crate::error::CoreError;

/// The in-system mount points equal to or below `in_system`, comparing
/// whole path components, in mountinfo order. Reads only mountinfo, never
/// the tree. An unreadable or unparsable mountinfo is an error: callers
/// fail closed.
pub fn mounts_at_or_below(env: &Env, in_system: &Path) -> Result<Vec<PathBuf>, CoreError> {
    let file = env.sys("/proc/self/mountinfo");
    let data = fs::read(&file).map_err(|source| CoreError::Io {
        path: file.clone(),
        source,
    })?;
    let mut found = Vec::new();
    for line in data.split(|b| *b == b'\n').filter(|l| !l.is_empty()) {
        let mount = parse_mount_point(line).ok_or_else(|| CoreError::Io {
            path: file.clone(),
            source: std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "unparsable mountinfo line",
            ),
        })?;
        let mount = env.unsys(&mount);
        if mount.starts_with(in_system) {
            found.push(mount);
        }
    }
    Ok(found)
}

/// Field 5 of a mountinfo line, unescaped; `None` when the line has too few
/// fields, the mount point is not absolute, or an escape is not one of
/// `\040`, `\011`, `\012`, `\134`.
fn parse_mount_point(line: &[u8]) -> Option<PathBuf> {
    let field = line.split(|b| *b == b' ').nth(4)?;
    let mut out = Vec::with_capacity(field.len());
    let mut i = 0;
    while i < field.len() {
        if field[i] == b'\\' {
            out.push(match field.get(i + 1..i + 4)? {
                b"040" => b' ',
                b"011" => b'\t',
                b"012" => b'\n',
                b"134" => b'\\',
                _ => return None,
            });
            i += 4;
        } else {
            out.push(field[i]);
            i += 1;
        }
    }
    (out.first() == Some(&b'/')).then(|| PathBuf::from(OsString::from_vec(out)))
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;
    use crate::testing::FixtureTree;

    fn line(mount: &str) -> String {
        format!("36 35 98:0 /mnt1 {mount} rw,noatime master:1 - ext3 /dev/root rw\n")
    }

    #[test]
    fn mountinfo_parsing_and_escapes() {
        let t = FixtureTree::new();
        let info = [
            line("/"),
            line("/opt/inky/data"),
            line(r"/opt/inky\040x"),
            line("/opt/inkyz"),
            line("/opt/inky"),
            line(r"/opt/inky/a\134b\011c\012d"),
            line("/opt"),
        ]
        .concat();
        t.write("proc/self/mountinfo", info.as_bytes());
        let env = t.env();
        assert_eq!(
            mounts_at_or_below(&env, Path::new("/opt/inky")).unwrap(),
            [
                PathBuf::from("/opt/inky/data"),
                PathBuf::from("/opt/inky"),
                PathBuf::from("/opt/inky/a\\b\tc\nd"),
            ]
        );
        assert_eq!(
            mounts_at_or_below(&env, Path::new("/opt/inky/x")).unwrap(),
            [] as [PathBuf; 0]
        );
    }

    #[test]
    fn space_escape_is_decoded() {
        let t = FixtureTree::new();
        t.write("proc/self/mountinfo", line(r"/opt/inky\040x").as_bytes());
        let env = t.env();
        assert_eq!(
            mounts_at_or_below(&env, Path::new("/opt/inky x")).unwrap(),
            [PathBuf::from("/opt/inky x")]
        );
        assert!(
            mounts_at_or_below(&env, Path::new("/opt/inky"))
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn fake_root_mount_points_map_to_in_system_paths() {
        let t = FixtureTree::new();
        let real = t.dir.path().join("opt/inky/data");
        t.write(
            "proc/self/mountinfo",
            line(&real.to_string_lossy()).as_bytes(),
        );
        let env = t.env();
        assert_eq!(
            mounts_at_or_below(&env, Path::new("/opt/inky")).unwrap(),
            [PathBuf::from("/opt/inky/data")]
        );
    }

    struct Restore(PathBuf);
    impl Drop for Restore {
        fn drop(&mut self) {
            let _ = fs::set_permissions(&self.0, fs::Permissions::from_mode(0o755));
        }
    }

    #[test]
    fn mount_check_does_not_read_the_tree() {
        let t = FixtureTree::new();
        let dir = t.mkdir("opt/inky");
        t.write("proc/self/mountinfo", line("/opt/inky/data").as_bytes());
        let _restore = Restore(dir.clone());
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o000)).unwrap();
        let env = t.env();
        assert_eq!(
            mounts_at_or_below(&env, Path::new("/opt/inky")).unwrap(),
            [PathBuf::from("/opt/inky/data")]
        );
    }

    #[test]
    fn missing_mountinfo_fails_closed() {
        let t = FixtureTree::new();
        let env = t.env();
        assert!(mounts_at_or_below(&env, Path::new("/opt/inky")).is_err());
    }

    #[test]
    fn unparsable_mountinfo_fails_closed() {
        for bad in [
            "36 35 98:0 /mnt1\n",
            "36 35 98:0 /mnt1 relative rw - ext3 /dev/root rw\n",
            "36 35 98:0 /mnt1 /opt/a\\777b rw - ext3 /dev/root rw\n",
            "36 35 98:0 /mnt1 /opt/a\\04 rw - ext3 /dev/root rw\n",
        ] {
            let t = FixtureTree::new();
            t.write("proc/self/mountinfo", bad.as_bytes());
            let env = t.env();
            assert!(
                mounts_at_or_below(&env, Path::new("/opt/inky")).is_err(),
                "{bad}"
            );
        }
    }
}
