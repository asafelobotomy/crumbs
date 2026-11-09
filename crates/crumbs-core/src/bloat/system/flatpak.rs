//! `flatpak-unused-system`: one item that has Flatpak remove the runtimes in
//! the system installation that no installed app uses. Opt-in.

use super::super::flatpak::{refs_note, unused_label, unused_refs};
use super::super::{BloatItem, Category, ScanContext, Scanner};
use super::{command_failed, not_installed, privileged_item};
use crate::command::resolve_trusted;
use crate::error::CoreError;

pub struct FlatpakUnusedSystemScanner;

impl Scanner for FlatpakUnusedSystemScanner {
    fn category(&self) -> Category {
        Category::FlatpakUnusedSystem
    }

    fn unavailable(&self, cx: &ScanContext) -> Option<String> {
        resolve_trusted(cx.env, "flatpak")
            .is_none()
            .then(|| not_installed("flatpak"))
    }

    fn scan(&self, cx: &ScanContext) -> Result<Vec<BloatItem>, CoreError> {
        let Some(flatpak) = resolve_trusted(cx.env, "flatpak") else {
            return Ok(Vec::new());
        };
        let out = cx.runner.run(
            &flatpak,
            &["list", "--system", "--runtime", "--columns=ref"],
        )?;
        if out.status != 0 {
            return Err(command_failed("flatpak", &out));
        }
        if !out.stdout.lines().any(|l| !l.trim().is_empty()) {
            return Ok(Vec::new());
        }
        // Listing needs no rights: flatpak shows its plan before it asks.
        let (label, note) = match unused_refs(cx, &flatpak, "--system") {
            Some(refs) if refs.is_empty() => return Ok(Vec::new()),
            Some(refs) => (unused_label(refs.len(), "system"), refs_note(&refs)),
            None => (
                "unused Flatpak runtimes (system)".into(),
                "decided by flatpak".into(),
            ),
        };
        let mut item = privileged_item(
            Category::FlatpakUnusedSystem,
            label,
            None,
            Some(note),
            "flatpak",
            [
                "uninstall",
                "--system",
                "--unused",
                "-y",
                "--noninteractive",
            ]
            .map(String::from)
            .to_vec(),
        );
        // Opt-in: listed, but only run when the category is named.
        item.preselected = false;
        Ok(vec![item])
    }
}
