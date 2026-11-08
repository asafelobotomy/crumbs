//! `system-temp`: one item that has `systemd-tmpfiles` clean what its
//! configuration says is stale. systemd decides what; crumbs cannot size it.

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
            Some("decided by systemd-tmpfiles".into()),
            "systemd-tmpfiles",
            vec!["--clean".into()],
        )])
    }
}
