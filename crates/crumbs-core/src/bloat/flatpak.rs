//! `flatpak-unused`: one command item that has Flatpak remove the runtimes
//! no installed app uses. Flatpak decides which ones; crumbs cannot size them.

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
        Ok(vec![BloatItem {
            category: Category::FlatpakUnused,
            path: None,
            label: "unused Flatpak runtimes (user)".into(),
            size: None,
            rebuildable: false,
            // The CLI preselects it when the category is named explicitly.
            preselected: false,
            note: Some("decided by flatpak".into()),
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
        assert_eq!(
            runner.calls(),
            ["flatpak list --user --runtime --columns=ref"]
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
