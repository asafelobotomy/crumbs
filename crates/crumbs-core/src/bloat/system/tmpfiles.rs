//! `system-temp`: one item that has `systemd-tmpfiles` clean what its
//! age rules say is stale, limited to `/tmp` and `/var/tmp`. systemd decides what; crumbs cannot size it.

use super::super::{BloatItem, Category, ScanContext, Scanner};
use super::{not_installed, privileged_item};
use crate::command::resolve_trusted;
use crate::error::CoreError;

pub struct SystemTempScanner;

impl Scanner for SystemTempScanner {
    fn category(&self) -> Category {
        Category::SystemTemp
    }

    fn unavailable(&self, cx: &ScanContext) -> Option<String> {
        resolve_trusted(cx.env, "systemd-tmpfiles")
            .is_none()
            .then(|| not_installed("systemd-tmpfiles"))
    }

    fn scan(&self, cx: &ScanContext) -> Result<Vec<BloatItem>, CoreError> {
        if resolve_trusted(cx.env, "systemd-tmpfiles").is_none() {
            return Ok(Vec::new());
        }
        Ok(vec![privileged_item(
            Category::SystemTemp,
            "stale system temporary files".into(),
            None,
            Some("removes files past the tmpfiles.d age limits under /tmp and /var/tmp".into()),
            "systemd-tmpfiles",
            vec![
                "--clean".into(),
                "--prefix=/tmp".into(),
                "--prefix=/var/tmp".into(),
            ],
        )])
    }
}
