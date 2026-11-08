//! `coredumps`: `core.*` files in systemd's coredump directory. Deleted as
//! root; the target must pass the `Scope::System` rules.

use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::Path;

use super::super::{BloatItem, Category, ItemAction, ScanContext, Scanner};
use crate::error::CoreError;
use crate::safety::{Scope, fingerprint, is_allowed_target};

const DIR: &str = "/var/lib/systemd/coredump";

pub struct CoredumpsScanner;

impl Scanner for CoredumpsScanner {
    fn category(&self) -> Category {
        Category::Coredumps
    }

    fn unavailable(&self, cx: &ScanContext) -> Option<String> {
        let is_dir = fs::symlink_metadata(cx.env.sys(DIR)).is_ok_and(|m| m.is_dir());
        (!is_dir).then(|| "no coredump directory".into())
    }

    fn scan(&self, cx: &ScanContext) -> Result<Vec<BloatItem>, CoreError> {
        let real = cx.env.sys(DIR);
        let rd = match fs::read_dir(&real) {
            Ok(rd) => rd,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(source) => {
                return Err(CoreError::Io {
                    path: Path::new(DIR).to_path_buf(),
                    source,
                });
            }
        };
        let mut names: Vec<_> = rd
            .flatten()
            .filter(|e| e.file_name().as_encoded_bytes().starts_with(b"core."))
            .map(|e| e.file_name())
            .collect();
        names.sort();
        let mut items = Vec::new();
        for name in names {
            let path = Path::new(DIR).join(&name);
            // lstat: a symlink or directory is never a coredump file.
            let Ok(meta) = fs::symlink_metadata(real.join(&name)) else {
                continue;
            };
            if !meta.is_file() || !is_allowed_target(cx.env, cx.cfg, Scope::System, &path) {
                continue;
            }
            let Ok(fp) = fingerprint(cx.env, &path) else {
                continue;
            };
            items.push(BloatItem {
                category: Category::Coredumps,
                path: Some(path),
                label: name.to_string_lossy().into_owned(),
                size: Some(meta.blocks() * 512),
                rebuildable: false,
                preselected: true,
                note: None,
                action: ItemAction::Delete,
                scope: Scope::System,
                fingerprint: Some(fp),
                extra_paths: Vec::new(),
                app_id: None,
            });
        }
        Ok(items)
    }
}
