//! Fedora/openSUSE packages, read with the trusted `rpm` (its database is
//! binary and never parsed directly).

use std::collections::HashMap;
use std::path::PathBuf;

use crate::command::{CommandRunner, resolve_trusted};
use crate::desktop::DesktopEntry;
use crate::env::Env;
use crate::error::CoreError;
use crate::model::{App, SourceKind};

use super::{AppSource, SourceContext, SourceListing};

/// One query for every package and its files: a `@@\t<name>\t<evr.arch>\t<size>`
/// header line per package, then one path per line.
pub(crate) const QUERY: &str =
    "@@\t%{NAME}\t%{EPOCHNUM}:%{VERSION}-%{RELEASE}.%{ARCH}\t%{SIZE}\n[%{FILENAMES}\n]";

pub struct RpmSource;

/// One package from the query output.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Package {
    pub name: String,
    /// `epoch:version-release`, the epoch left out when it is 0.
    pub version: String,
    pub size: Option<u64>,
    pub files: Vec<PathBuf>,
}

/// Runs the query and parses it. A non-zero exit, or output that could not
/// be decoded exactly (a replacement character), is an error: a partial or
/// altered file list would report owned paths as unowned.
pub(crate) fn query(env: &Env, runner: &dyn CommandRunner) -> Result<Vec<Package>, CoreError> {
    let rpm = resolve_trusted(env, "rpm").ok_or_else(|| CoreError::Command {
        program: "rpm".into(),
        message: "not installed".into(),
    })?;
    let out = runner.run(&rpm, &["-qa", "--qf", QUERY])?;
    if out.status != 0 {
        return Err(CoreError::Command {
            program: "rpm".into(),
            message: format!(
                "rpm -qa failed (status {}): {}",
                out.status,
                out.stderr.trim()
            ),
        });
    }
    if out.stdout.contains('\u{fffd}') {
        return Err(CoreError::Command {
            program: "rpm".into(),
            message: "rpm listed a file name that is not valid UTF-8".into(),
        });
    }
    Ok(parse(&out.stdout))
}

/// Parses the query output; `gpg-pubkey` entries (keys, not packages) are
/// skipped.
pub(crate) fn parse(text: &str) -> Vec<Package> {
    let mut out: Vec<Package> = Vec::new();
    let mut skip = false;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("@@\t") {
            let mut f = rest.split('\t');
            let name = f.next().unwrap_or("").to_string();
            let evra = f.next().unwrap_or("");
            let size = f.next().and_then(|s| s.trim().parse().ok());
            skip = name.is_empty() || name == "gpg-pubkey" || evra.ends_with(".(none)");
            if !skip {
                out.push(Package {
                    name,
                    version: version(evra),
                    size,
                    files: Vec::new(),
                });
            }
        } else if line.starts_with('/')
            && !skip
            && let Some(p) = out.last_mut()
        {
            p.files.push(PathBuf::from(line));
        }
    }
    out
}

/// `0:1.2-3.fc41.x86_64` becomes `1.2-3.fc41`; a non-zero epoch stays.
fn version(evra: &str) -> String {
    let evr = evra.rsplit_once('.').map_or(evra, |(v, _arch)| v);
    evr.strip_prefix("0:").unwrap_or(evr).to_string()
}

impl AppSource for RpmSource {
    fn kind(&self) -> SourceKind {
        SourceKind::Rpm
    }

    fn available(&self, env: &Env) -> bool {
        crate::ownership::rpm_present(env)
    }

    fn is_package_db(&self) -> bool {
        true
    }

    fn list(&self, cx: &SourceContext) -> Result<SourceListing, CoreError> {
        let packages = query(cx.env, cx.runner)?;
        let by_id: HashMap<&str, &DesktopEntry> =
            cx.desktop.iter().map(|d| (d.id.as_str(), d)).collect();
        let mut listing = SourceListing::default();
        for p in packages {
            let mut app = App::new(SourceKind::Rpm, &p.name);
            app.version = Some(p.version);
            app.install_size = p.size;
            super::link_desktop(&mut app, &p.files, &by_id, &mut listing.claimed_desktop_ids);
            listing.owned_paths.extend(p.files);
            if app.has_gui || cx.opts.include_all {
                listing.apps.push(app);
            }
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
    use crate::ownership::load_package_db;
    use crate::sources::{AppSource, ListOptions, SourceContext};
    use crate::testing::{FakeRunner, FixtureTree};
    use std::path::Path;

    const QA: &str = include_str!("../../tests/fixtures/rpm/qa");
    const FIREFOX_DESKTOP: &str =
        "[Desktop Entry]\nType=Application\nName=Firefox\nExec=/usr/lib64/firefox/firefox %u\n";

    fn ok(stdout: &str) -> CmdOutput {
        CmdOutput {
            status: 0,
            stdout: stdout.into(),
            stderr: String::new(),
            timed_out: false,
        }
    }

    fn fedora() -> FixtureTree {
        let t = FixtureTree::new();
        t.write_exec("/usr/bin/rpm", b"#!/bin/sh\n");
        t.write("/usr/lib/sysimage/rpm/rpmdb.sqlite", b"x");
        t.write(
            "/usr/share/applications/firefox.desktop",
            FIREFOX_DESKTOP.as_bytes(),
        );
        t
    }

    fn runner(out: CmdOutput) -> FakeRunner {
        FakeRunner::new().on("rpm", &["-qa", "--qf", QUERY], out)
    }

    fn list(t: &FixtureTree, r: &FakeRunner, include_all: bool) -> SourceListing {
        let env = t.env();
        let desktop = discover_desktop_entries(&env);
        let opts = ListOptions { include_all };
        let cx = SourceContext {
            env: &env,
            runner: r,
            desktop: &desktop,
            opts: &opts,
            config: &Config::default(),
        };
        RpmSource.list(&cx).unwrap()
    }

    #[test]
    fn rpm_lists_packages_from_query() {
        let t = fedora();
        let r = runner(ok(QA));
        let l = list(&t, &r, false);
        let ids: Vec<&str> = l.apps.iter().map(|a| a.id.as_str()).collect();
        assert_eq!(ids, ["rpm:firefox"]);
        assert_eq!(l.apps[0].desktop_ids, ["firefox.desktop"]);
        assert_eq!(l.apps[0].install_size, Some(280_000_000));
        let all = list(&t, &r, true);
        let names: Vec<&str> = all.apps.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, ["coreutils", "firefox", "shadow-utils"]);
        assert!(all.owned_paths.contains(Path::new("/usr/bin/arch")));
        assert!(l.errors.is_empty(), "{:?}", l.errors);
    }

    #[test]
    fn rpm_epoch_zero_omitted_from_version() {
        let all = list(&fedora(), &runner(ok(QA)), true);
        let v = |n: &str| {
            all.apps
                .iter()
                .find(|a| a.name == n)
                .unwrap()
                .version
                .clone()
        };
        assert_eq!(v("coreutils").as_deref(), Some("9.10-5.fc44"));
        assert_eq!(v("shadow-utils").as_deref(), Some("2:4.19.0-7.fc44"));
    }

    #[test]
    fn rpm_query_failure_is_source_error_and_index_incomplete() {
        let t = fedora();
        let failing = CmdOutput {
            status: 1,
            stdout: String::new(),
            stderr: "error: rpmdb open failed".into(),
            timed_out: false,
        };
        let env = t.env();
        let desktop = discover_desktop_entries(&env);
        let r = runner(failing.clone());
        let cx = SourceContext {
            env: &env,
            runner: &r,
            desktop: &desktop,
            opts: &ListOptions::default(),
            config: &Config::default(),
        };
        assert!(RpmSource.list(&cx).is_err());
        assert!(load_package_db(&env, &runner(failing)).is_err());
    }

    #[test]
    fn rpm_filenames_include_dirs() {
        let t = fedora();
        let idx = load_package_db(&t.env(), &runner(ok(QA))).unwrap().unwrap();
        assert!(idx.is_complete());
        assert!(idx.owns(Path::new("/usr/lib64/firefox")));
        assert!(idx.owns_at_or_below(Path::new("/usr/lib64/firefox")));
        assert!(!idx.owns_at_or_below(Path::new("/opt/inky")));
    }

    #[test]
    fn rpm_lossy_output_makes_index_incomplete() {
        let t = fedora();
        let lossy = format!("{QA}/usr/share/caf\u{fffd}\n");
        let idx = load_package_db(&t.env(), &runner(ok(&lossy)));
        assert!(idx.is_err());
    }
}
