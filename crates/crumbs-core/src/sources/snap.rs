//! Snap packages, listed with the trusted `snap`.

use std::collections::HashMap;

use crate::command::resolve_trusted;
use crate::desktop::{DesktopEntry, exec_basename};
use crate::env::Env;
use crate::error::CoreError;
use crate::model::{App, Installation, SourceKind};

use super::{AppSource, SourceContext, SourceListing};

/// Notes that mark a snap as part of the platform, not an app.
const PLATFORM_NOTES: [&str; 4] = ["base", "core", "snapd", "gadget"];

pub struct SnapSource;

/// `(name, version)` of the app snaps in `snap list --unicode=never` output.
pub(crate) fn parse_list(text: &str) -> Vec<(String, String)> {
    text.lines()
        .skip(1)
        .filter_map(|line| {
            let cols: Vec<&str> = line.split_whitespace().collect();
            if cols.len() < 6 {
                return None;
            }
            let notes = cols[cols.len() - 1];
            if notes.split(',').any(|n| PLATFORM_NOTES.contains(&n)) {
                return None;
            }
            Some((cols[0].to_string(), cols[1].to_string()))
        })
        .collect()
}

impl AppSource for SnapSource {
    fn kind(&self) -> SourceKind {
        SourceKind::Snap
    }

    fn available(&self, env: &Env) -> bool {
        resolve_trusted(env, "snap").is_some()
    }

    fn is_package_db(&self) -> bool {
        false
    }

    fn list(&self, cx: &SourceContext) -> Result<SourceListing, CoreError> {
        let snap = resolve_trusted(cx.env, "snap").ok_or_else(|| CoreError::Command {
            program: "snap".into(),
            message: "not installed".into(),
        })?;
        let out = cx.runner.run(&snap, &["list", "--unicode=never"])?;
        if out.status != 0 {
            return Err(CoreError::Command {
                program: "snap".into(),
                message: format!(
                    "snap list failed (status {}): {}",
                    out.status,
                    out.stderr.trim()
                ),
            });
        }
        let mut by_snap: HashMap<&str, Vec<&DesktopEntry>> = HashMap::new();
        for d in cx.desktop {
            if let Some((name, _app)) = d.id.split_once('_') {
                by_snap.entry(name).or_default().push(d);
            }
        }
        let mut listing = SourceListing::default();
        for (name, version) in parse_list(&out.stdout) {
            let mut app = App::new(SourceKind::Snap, &name);
            app.version = Some(version);
            app.installation = Installation::System;
            app.has_gui = true;
            for e in by_snap.get(name.as_str()).into_iter().flatten() {
                app.desktop_ids.push(e.id.clone());
                app.exec_names.extend(exec_basename(e));
                app.wm_classes.extend(e.wm_class.clone());
                listing.claimed_desktop_ids.insert(e.id.clone());
            }
            if let Some(n) = by_snap
                .get(name.as_str())
                .and_then(|v| v.first())
                .and_then(|e| e.name.clone())
            {
                app.display_name = n;
            }
            listing.apps.push(app);
        }
        listing.apps.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(listing)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::CmdOutput;
    use crate::config::Config;
    use crate::desktop::discover_desktop_entries;
    use crate::sources::{AppSource, ListOptions, SourceContext};
    use crate::testing::{FakeRunner, FixtureTree};

    const LIST: &str = include_str!("../../tests/fixtures/snap/list");

    fn ok(stdout: &str) -> CmdOutput {
        CmdOutput {
            status: 0,
            stdout: stdout.into(),
            stderr: String::new(),
            timed_out: false,
        }
    }

    fn listing(t: &FixtureTree, r: &FakeRunner) -> SourceListing {
        let env = t.env();
        let desktop = discover_desktop_entries(&env);
        let opts = ListOptions::default();
        let cx = SourceContext {
            env: &env,
            runner: r,
            desktop: &desktop,
            opts: &opts,
            config: &Config::default(),
        };
        SnapSource.list(&cx).unwrap()
    }

    #[test]
    fn snap_list_skips_bases_and_snapd() {
        let t = FixtureTree::new();
        t.write_exec("/usr/bin/snap", b"#!/bin/sh\n");
        t.write(
            "/usr/share/applications/firefox_firefox.desktop",
            b"[Desktop Entry]\nType=Application\nName=Firefox\nExec=/snap/bin/firefox\n",
        );
        let r = FakeRunner::new().on("snap", &["list", "--unicode=never"], ok(LIST));
        let l = listing(&t, &r);
        let ids: Vec<&str> = l.apps.iter().map(|a| a.id.as_str()).collect();
        assert_eq!(
            ids,
            ["snap:firefox", "snap:gtk-common-themes", "snap:spotify"]
        );
        let ff = &l.apps[0];
        assert_eq!(ff.version.as_deref(), Some("145.0-1"));
        assert_eq!(ff.desktop_ids, ["firefox_firefox.desktop"]);
        assert_eq!(ff.display_name, "Firefox");
        assert!(l.claimed_desktop_ids.contains("firefox_firefox.desktop"));
        assert!(ff.has_gui);
    }

    #[test]
    fn snap_list_failure_is_an_error() {
        let t = FixtureTree::new();
        t.write_exec("/usr/bin/snap", b"#!/bin/sh\n");
        let failing = CmdOutput {
            status: 1,
            stdout: String::new(),
            stderr: "error: cannot communicate with server".into(),
            timed_out: false,
        };
        let r = FakeRunner::new().on("snap", &["list", "--unicode=never"], failing);
        let env = t.env();
        let opts = ListOptions::default();
        let cx = SourceContext {
            env: &env,
            runner: &r,
            desktop: &[],
            opts: &opts,
            config: &Config::default(),
        };
        assert!(SnapSource.list(&cx).is_err());
    }
}
