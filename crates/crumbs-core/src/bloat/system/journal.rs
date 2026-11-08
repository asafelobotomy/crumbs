//! `journal`: the systemd journal, vacuumed down to `journal_max` when it
//! has grown past it.

use super::super::{BloatItem, Category, ScanContext, Scanner};
use super::{command_failed, not_installed, parse_size, privileged_item};
use crate::command::resolve_trusted;
use crate::config::journal_size_bytes;
use crate::error::CoreError;

/// Used when the configured limit cannot be read (the loader already
/// replaces an invalid one, but a hand-built `Config` might not).
const DEFAULT_MAX: u64 = 500 << 20;

pub struct JournalScanner;

impl Scanner for JournalScanner {
    fn category(&self) -> Category {
        Category::Journal
    }

    fn unavailable(&self, cx: &ScanContext) -> Option<String> {
        resolve_trusted(cx.env, "journalctl")
            .is_none()
            .then(|| not_installed("journalctl"))
    }

    fn scan(&self, cx: &ScanContext) -> Result<Vec<BloatItem>, CoreError> {
        let Some(journalctl) = resolve_trusted(cx.env, "journalctl") else {
            return Ok(Vec::new());
        };
        let out = cx.runner.run(&journalctl, &["--disk-usage"])?;
        if out.status != 0 {
            return Err(command_failed("journalctl", &out));
        }
        let usage = out
            .stdout
            .lines()
            .find_map(|l| {
                let rest = l.split_once("take up ")?.1;
                parse_size(rest.split_whitespace().next()?)
            })
            .ok_or_else(|| CoreError::Parse {
                what: "journalctl --disk-usage".into(),
                message: "no `take up <size>` text".into(),
            })?;
        let max = journal_size_bytes(&cx.cfg.journal_max).unwrap_or(DEFAULT_MAX);
        if usage <= max {
            return Ok(Vec::new());
        }
        Ok(vec![privileged_item(
            Category::Journal,
            "systemd journal".into(),
            Some(usage - max),
            Some(format!("vacuums to {}", cx.cfg.journal_max)),
            "journalctl",
            vec![format!("--vacuum-size={}", cx.cfg.journal_max)],
        )])
    }
}
