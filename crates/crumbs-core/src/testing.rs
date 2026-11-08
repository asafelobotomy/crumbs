//! Fixture helpers for tests. Not part of the production API.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fs;
use std::path::PathBuf;

use crate::env::Env;

pub struct FixtureTree {
    pub dir: tempfile::TempDir,
}

impl Default for FixtureTree {
    fn default() -> Self {
        Self::new()
    }
}

impl FixtureTree {
    pub fn new() -> Self {
        FixtureTree {
            dir: tempfile::tempdir().expect("create temp dir"),
        }
    }

    /// Builds an `Env` rooted at this tree; re-reads os-release each call.
    pub fn env(&self) -> Env {
        Env::fake(self.dir.path()).expect("fake env")
    }

    fn real(&self, rel: &str) -> PathBuf {
        self.dir.path().join(rel.trim_start_matches('/'))
    }

    pub fn write(&self, rel: &str, contents: &[u8]) -> PathBuf {
        let p = self.real(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(&p, contents).unwrap();
        p
    }

    pub fn symlink(&self, rel: &str, target: &str) {
        let p = self.real(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(target, &p).unwrap();
    }

    pub fn mkdir(&self, rel: &str) -> PathBuf {
        let p = self.real(rel);
        fs::create_dir_all(&p).unwrap();
        p
    }

    pub fn os_release(&self, id: &str, id_like: &str, name: &str) {
        let body = format!("NAME=\"{name}\"\nID={id}\nID_LIKE={id_like}\n");
        self.write("/etc/os-release", body.as_bytes());
    }

    /// Writes a pacman local-db entry (`desc` and `files`) for a package.
    pub fn pacman_pkg(&self, name: &str, version: &str, size: Option<u64>, files: &[&str]) {
        let dir = format!("/var/lib/pacman/local/{name}-{version}");
        let mut desc = format!("%NAME%\n{name}\n\n%VERSION%\n{version}\n\n");
        if let Some(s) = size {
            desc.push_str(&format!("%SIZE%\n{s}\n\n"));
        }
        self.write(&format!("{dir}/desc"), desc.as_bytes());
        let mut list = String::from("%FILES%\n");
        for f in files {
            list.push_str(f);
            list.push('\n');
        }
        self.write(&format!("{dir}/files"), list.as_bytes());
    }

    pub fn desktop(&self, rel_path: &str, contents: &str) {
        self.write(rel_path, contents.as_bytes());
    }

    pub fn appimage(&self, rel_path: &str) {
        let mut b = vec![0x7f, b'E', b'L', b'F', 0, 0, 0, 0, b'A', b'I', 0x02];
        b.resize(256, 0);
        self.write(rel_path, &b);
    }
}
