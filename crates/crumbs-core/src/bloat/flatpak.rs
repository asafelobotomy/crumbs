//! `flatpak-unused`: one command item that has Flatpak remove the runtimes
//! no installed app uses. Flatpak decides which ones; crumbs asks it first
//! (see [`unused_refs`]) and cannot size them.

use std::path::Path;

use super::{BloatItem, Category, ItemAction, ScanContext, Scanner};
use crate::command::resolve_trusted;
use crate::error::CoreError;
use crate::safety::Scope;

pub struct FlatpakUnusedScanner;

impl Scanner for FlatpakUnusedScanner {
    fn category(&self) -> Category {
        Category::FlatpakUnused
    }

    fn scan(&self, cx: &ScanContext) -> Result<Vec<BloatItem>, CoreError> {
        // No trusted flatpak: the category is unavailable, not broken.
        let Some(flatpak) = resolve_trusted(cx.env, "flatpak") else {
            return Ok(Vec::new());
        };
        let out = cx
            .runner
            .run(&flatpak, &["list", "--user", "--runtime", "--columns=ref"])?;
        if out.status != 0 {
            let detail = out.stderr.trim();
            return Err(CoreError::Command {
                program: "flatpak".into(),
                message: if detail.is_empty() {
                    format!("exited with status {}", out.status)
                } else {
                    detail.to_string()
                },
            });
        }
        if !out.stdout.lines().any(|l| !l.trim().is_empty()) {
            return Ok(Vec::new());
        }
        let (label, note) = match unused_refs(cx, &flatpak, "--user") {
            Some(refs) if refs.is_empty() => return Ok(Vec::new()),
            Some(refs) => (unused_label(refs.len(), "user"), refs_note(&refs)),
            None => (
                "unused Flatpak runtimes (user)".into(),
                "decided by flatpak".into(),
            ),
        };
        Ok(vec![BloatItem {
            category: Category::FlatpakUnused,
            path: None,
            label,
            size: None,
            rebuildable: false,
            // The CLI preselects it when the category is named explicitly.
            preselected: false,
            note: Some(note),
            action: ItemAction::Command {
                program: "flatpak".into(),
                args: ["uninstall", "--user", "--unused", "--noninteractive", "-y"]
                    .map(String::from)
                    .to_vec(),
            },
            scope: Scope::Home,
            fingerprint: None,
            extra_paths: Vec::new(),
            app_id: None,
        }])
    }
}

/// What `flatpak uninstall <installation> --unused` would remove, as
/// `id//branch`. Run without `-y`, flatpak prints its list and then, with
/// stdout not a terminal, answers its own prompt "n" and removes nothing
/// (`flatpak_yes_no_prompt`, flatpak 1.x). Only asked of a flatpak that
/// reports a 1.x version, where that is checked; `None` otherwise, and
/// when the answer is not one this understands: then flatpak decides at
/// clean time, as before.
pub(super) fn unused_refs(
    cx: &ScanContext,
    flatpak: &Path,
    installation: &str,
) -> Option<Vec<String>> {
    let version = cx.runner.run(flatpak, &["--version"]).ok()?;
    if version.status != 0 || !version.stdout.trim().starts_with("Flatpak 1.") {
        return None;
    }
    let out = cx
        .runner
        .run(flatpak, &["uninstall", installation, "--unused"])
        .ok()?;
    if out.timed_out {
        // Output cut short could undercount.
        return None;
    }
    parse_unused(&out.stdout)
}

/// The refs in flatpak's uninstall table (rows `N. <id> [<arch>] <branch>
/// r`), `Some(empty)` for `Nothing unused to uninstall`, `None` otherwise.
fn parse_unused(stdout: &str) -> Option<Vec<String>> {
    if stdout
        .lines()
        .any(|l| l.trim() == "Nothing unused to uninstall")
    {
        return Some(Vec::new());
    }
    let mut refs = Vec::new();
    for line in stdout.lines() {
        let t: Vec<&str> = line.split_whitespace().collect();
        let Some(first) = t.first() else { continue };
        let numbered = first.len() > 1
            && first.ends_with('.')
            && first[..first.len() - 1].bytes().all(|b| b.is_ascii_digit());
        if !numbered {
            continue;
        }
        // N. id [arch] branch op
        if !(4..=5).contains(&t.len()) || t[t.len() - 1] != "r" {
            return None;
        }
        refs.push(format!("{}//{}", t[1], t[t.len() - 2]));
    }
    (!refs.is_empty()).then_some(refs)
}

/// `2 unused Flatpak runtimes (user)`.
pub(super) fn unused_label(n: usize, installation: &str) -> String {
    let what = if n == 1 { "runtime" } else { "runtimes" };
    format!("{n} unused Flatpak {what} ({installation})")
}

/// The refs, the first few by name.
pub(super) fn refs_note(refs: &[String]) -> String {
    const SHOWN: usize = 4;
    let mut note = refs
        .iter()
        .take(SHOWN)
        .cloned()
        .collect::<Vec<_>>()
        .join(", ");
    if refs.len() > SHOWN {
        note.push_str(&format!(" and {} more", refs.len() - SHOWN));
    }
    note
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::CmdOutput;
    use crate::config::Config;
    use crate::inventory::Inventory;
    use crate::testing::{FakeRunner, FixtureTree};
    use std::time::SystemTime;

    const LIST: [&str; 4] = ["list", "--user", "--runtime", "--columns=ref"];

    fn out(status: i32, stdout: &str, stderr: &str) -> CmdOutput {
        CmdOutput {
            status,
            stdout: stdout.into(),
            stderr: stderr.into(),
            timed_out: false,
        }
    }

    fn scan(t: &FixtureTree, runner: &FakeRunner) -> Result<Vec<BloatItem>, CoreError> {
        let env = t.env();
        let cfg = Config::default();
        let inv = Inventory {
            apps: vec![],
            statuses: vec![],
            errors: vec![],
            owned: Default::default(),
        };
        let cx = ScanContext {
            env: &env,
            cfg: &cfg,
            inv: &inv,
            runner,
            now: SystemTime::now(),
        };
        FlatpakUnusedScanner.scan(&cx)
    }

    #[test]
    fn flatpak_unused_item_when_runtimes() {
        let t = FixtureTree::new();
        t.write_exec("usr/bin/flatpak", b"#!/bin/sh\n");
        let runner = FakeRunner::new().on(
            "flatpak",
            &LIST,
            out(0, "org.freedesktop.Platform/x86_64/23.08\n", ""),
        );
        let items = scan(&t, &runner).unwrap();
        assert_eq!(items.len(), 1);
        let i = &items[0];
        assert_eq!(i.category, Category::FlatpakUnused);
        assert_eq!(i.path, None);
        assert_eq!(i.label, "unused Flatpak runtimes (user)");
        assert_eq!(i.size, None);
        assert_eq!(i.note.as_deref(), Some("decided by flatpak"));
        assert!(!i.rebuildable && !i.preselected);
        assert_eq!(
            i.action,
            ItemAction::Command {
                program: "flatpak".into(),
                args: ["uninstall", "--user", "--unused", "--noninteractive", "-y"]
                    .map(String::from)
                    .to_vec(),
            }
        );
        assert_eq!(i.scope, Scope::Home);
        assert!(i.fingerprint.is_none() && i.extra_paths.is_empty() && i.app_id.is_none());
        // No 1.x version to rely on: never probed, flatpak decides as before.
        assert_eq!(
            runner.calls(),
            [
                "flatpak list --user --runtime --columns=ref",
                "flatpak --version"
            ]
        );
    }

    const PROBE: [&str; 3] = ["uninstall", "--user", "--unused"];

    /// Flatpak's table for two unused runtimes, as it prints it before
    /// declining (stdout is not a terminal), with a pinned notice.
    const TABLE: &str = "\nThese runtimes in installation 'user' are pinned and won't be removed; \
        see flatpak-pin(1):\n  runtime/org.kde.Pinned/x86_64/6\n\n\n        ID                        \
        Branch    Op\n 1.     org.gnome.Platform        45        r\n 2.     \
        org.freedesktop.Platform.GL.default  23.08  r\n\nProceed with these changes to the user \
        installation? [Y/n]: n\n";

    #[test]
    fn parse_unused_reads_flatpak_answers() {
        assert_eq!(parse_unused("Nothing unused to uninstall\n"), Some(vec![]));
        let pinned = "\nThese runtimes in installation 'system' are pinned and won't be removed; \
            see flatpak-pin(1):\n  runtime/x/x86_64/stable\nNothing unused to uninstall\n";
        assert_eq!(parse_unused(pinned), Some(vec![]));
        assert_eq!(
            parse_unused(TABLE),
            Some(vec![
                "org.gnome.Platform//45".to_string(),
                "org.freedesktop.Platform.GL.default//23.08".to_string()
            ])
        );
        // With the Arch column (refs of more than one architecture).
        let arches = " 1.     org.gnome.Platform   x86_64  45  r\n 2.     org.gnome.Platform   i386  45  r\n";
        assert_eq!(
            parse_unused(arches),
            Some(vec![
                "org.gnome.Platform//45".to_string(),
                "org.gnome.Platform//45".to_string()
            ])
        );
        // Anything else: not understood.
        assert_eq!(parse_unused(""), None);
        assert_eq!(parse_unused("error: something\n"), None);
        assert_eq!(parse_unused(" 1.     org.gnome.Platform   45  i\n"), None);
    }

    #[test]
    fn flatpak_unused_none_when_flatpak_finds_none() {
        let t = FixtureTree::new();
        t.write_exec("usr/bin/flatpak", b"#!/bin/sh\n");
        let runner = FakeRunner::new()
            .on(
                "flatpak",
                &LIST,
                out(0, "org.freedesktop.Platform/x86_64/23.08\n", ""),
            )
            .on("flatpak", &["--version"], out(0, "Flatpak 1.18.4\n", ""))
            .on(
                "flatpak",
                &PROBE,
                out(0, "Nothing unused to uninstall\n", ""),
            );
        assert!(scan(&t, &runner).unwrap().is_empty());
        // A flatpak of another major version is never probed.
        let runner = FakeRunner::new()
            .on(
                "flatpak",
                &LIST,
                out(0, "org.freedesktop.Platform/x86_64/23.08\n", ""),
            )
            .on("flatpak", &["--version"], out(0, "Flatpak 2.0.0\n", ""))
            .on(
                "flatpak",
                &PROBE,
                out(0, "Nothing unused to uninstall\n", ""),
            );
        let items = scan(&t, &runner).unwrap();
        assert_eq!(items[0].label, "unused Flatpak runtimes (user)");
        assert!(!runner.calls().iter().any(|c| c.contains("uninstall")));
    }

    #[test]
    fn flatpak_unused_probe_cut_off_by_timeout_is_not_trusted() {
        // A table cut short would undercount: flatpak decides, as before.
        let t = FixtureTree::new();
        t.write_exec("usr/bin/flatpak", b"#!/bin/sh\n");
        let mut cut = out(1, " 1.     org.gnome.Platform   45   r\n", "");
        cut.timed_out = true;
        let runner = FakeRunner::new()
            .on(
                "flatpak",
                &LIST,
                out(0, "org.gnome.Platform/x86_64/45\n", ""),
            )
            .on("flatpak", &["--version"], out(0, "Flatpak 1.18.4\n", ""))
            .on("flatpak", &PROBE, cut);
        let items = scan(&t, &runner).unwrap();
        assert_eq!(items[0].label, "unused Flatpak runtimes (user)");
        assert_eq!(items[0].note.as_deref(), Some("decided by flatpak"));
    }

    #[test]
    fn flatpak_unused_names_what_flatpak_would_remove() {
        let t = FixtureTree::new();
        t.write_exec("usr/bin/flatpak", b"#!/bin/sh\n");
        let runner = FakeRunner::new()
            .on(
                "flatpak",
                &LIST,
                out(0, "org.gnome.Platform/x86_64/45\n", ""),
            )
            .on("flatpak", &["--version"], out(0, "Flatpak 1.18.4\n", ""))
            // Declining exits non-zero; the list is what counts.
            .on("flatpak", &PROBE, out(1, TABLE, "error: Aborted by user\n"));
        let items = scan(&t, &runner).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].label, "2 unused Flatpak runtimes (user)");
        assert_eq!(
            items[0].note.as_deref(),
            Some("org.gnome.Platform//45, org.freedesktop.Platform.GL.default//23.08")
        );
    }

    #[test]
    fn flatpak_unused_none_when_no_runtimes() {
        let t = FixtureTree::new();
        t.write_exec("usr/bin/flatpak", b"#!/bin/sh\n");
        let runner = FakeRunner::new().on("flatpak", &LIST, out(0, "\n  \n", ""));
        assert!(scan(&t, &runner).unwrap().is_empty());
    }

    #[test]
    fn flatpak_missing_is_unavailable_not_error() {
        let t = FixtureTree::new();
        let runner = FakeRunner::new();
        assert!(scan(&t, &runner).unwrap().is_empty());
        assert!(runner.calls().is_empty());
    }

    #[test]
    fn flatpak_nonzero_exit_is_error() {
        let t = FixtureTree::new();
        t.write_exec("usr/bin/flatpak", b"#!/bin/sh\n");
        let runner = FakeRunner::new().on("flatpak", &LIST, out(1, "", "boom\n"));
        let err = scan(&t, &runner).unwrap_err();
        assert!(matches!(err, CoreError::Command { .. }));
        assert!(err.to_string().contains("boom"));
    }
}
