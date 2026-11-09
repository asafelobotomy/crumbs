//! Debian/Ubuntu packages, read straight from the dpkg database.

use std::collections::HashMap;
use std::ffi::OsStr;
use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use crate::desktop::DesktopEntry;
use crate::env::Env;
use crate::error::{CoreError, SourceError};
use crate::model::{App, SourceKind};

use super::{AppSource, SourceContext, SourceListing};

pub(crate) const STATUS: &str = "/var/lib/dpkg/status";
pub(crate) const INFO_DIR: &str = "/var/lib/dpkg/info";
const INSTALLED: &str = "install ok installed";

pub struct DpkgSource;

/// One stanza of `/var/lib/dpkg/status`.
#[derive(Debug, Default, Clone, PartialEq)]
pub(crate) struct Stanza {
    pub name: String,
    pub status: String,
    pub version: Option<String>,
    pub arch: Option<String>,
    /// `Installed-Size`, in KiB.
    pub installed_kib: Option<u64>,
    pub description: Option<String>,
}

impl Stanza {
    pub fn installed(&self) -> bool {
        self.status == INSTALLED
    }
}

/// Parses the status file. Continuation lines (leading space) belong to the
/// previous field and are ignored except for the first description line.
pub(crate) fn parse_status(text: &str) -> Vec<Stanza> {
    let mut out = Vec::new();
    let mut cur = Stanza::default();
    for line in text.lines() {
        if line.is_empty() {
            if !cur.name.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
            continue;
        }
        if line.starts_with([' ', '\t']) {
            continue;
        }
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim();
        match key {
            "Package" => cur.name = value.to_string(),
            "Status" => cur.status = value.to_string(),
            "Version" => cur.version = Some(value.to_string()),
            "Architecture" => cur.arch = Some(value.to_string()),
            "Installed-Size" => cur.installed_kib = value.parse().ok(),
            "Description" => cur.description = Some(value.to_string()),
            _ => {}
        }
    }
    if !cur.name.is_empty() {
        out.push(cur);
    }
    out
}

/// The paths of one `.list` file, as in-system absolute paths. dpkg lists
/// directories as plain paths; `/.` (the root) is skipped.
pub(crate) fn list_paths(bytes: &[u8]) -> Vec<PathBuf> {
    bytes
        .split(|&b| b == b'\n')
        .filter(|l| !l.is_empty() && *l != b"/." && l.starts_with(b"/"))
        .map(|l| PathBuf::from(OsStr::from_bytes(l)))
        .collect()
}

/// Lists read from the info directory, and the files that could not be read.
type Lists = (
    HashMap<String, Vec<PathBuf>>,
    Vec<(PathBuf, std::io::Error)>,
);

/// Every `.list` file in the info directory, by package name (the part
/// before `:` for multiarch names), with its paths.
fn read_lists(env: &Env) -> Lists {
    let dir = env.sys(INFO_DIR);
    let mut lists: HashMap<String, Vec<PathBuf>> = HashMap::new();
    let mut errors = Vec::new();
    let entries = match fs::read_dir(&dir) {
        Ok(e) => e,
        Err(e) => {
            errors.push((dir, e));
            return (lists, errors);
        }
    };
    for entry in entries {
        let path = match entry {
            Ok(e) => e.path(),
            Err(e) => {
                errors.push((dir.clone(), e));
                continue;
            }
        };
        if path.extension() != Some(OsStr::new("list")) {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        let name = stem.split(':').next().unwrap_or(stem).to_string();
        match fs::read(&path) {
            Ok(b) => lists.entry(name).or_default().extend(list_paths(&b)),
            Err(e) => errors.push((path, e)),
        }
    }
    (lists, errors)
}

/// Every path the dpkg database lists as owned. Any unreadable list is an
/// error, like [`crate::ownership::load_pacman`].
pub(crate) fn owned_paths(env: &Env) -> Result<Vec<PathBuf>, CoreError> {
    let (lists, errors) = read_lists(env);
    if let Some((path, source)) = errors.into_iter().next() {
        return Err(CoreError::Io { path, source });
    }
    Ok(lists.into_values().flatten().collect())
}

impl AppSource for DpkgSource {
    fn kind(&self) -> SourceKind {
        SourceKind::Dpkg
    }

    fn available(&self, env: &Env) -> bool {
        env.sys(STATUS).is_file()
    }

    fn is_package_db(&self) -> bool {
        true
    }

    fn list(&self, cx: &SourceContext) -> Result<SourceListing, CoreError> {
        let status_path = cx.env.sys(STATUS);
        let bytes = fs::read(&status_path).map_err(|source| CoreError::Io {
            path: status_path.clone(),
            source,
        })?;
        let stanzas = parse_status(&String::from_utf8_lossy(&bytes));
        let (mut lists, errors) = read_lists(cx.env);
        let mut listing = SourceListing::default();
        for (path, e) in errors {
            listing.errors.push(SourceError {
                source: SourceKind::Dpkg,
                message: format!("cannot read {}: {e}", display(&cx.env.unsys(&path))),
            });
        }
        let by_id: HashMap<&str, &DesktopEntry> =
            cx.desktop.iter().map(|d| (d.id.as_str(), d)).collect();
        for st in stanzas.iter().filter(|s| s.installed()) {
            let files = lists.remove(&st.name).unwrap_or_default();
            let mut app = App::new(SourceKind::Dpkg, &st.name);
            app.version = st.version.clone();
            app.description = st.description.clone();
            app.install_size = st.installed_kib.map(|k| k * 1024);
            super::link_desktop(&mut app, &files, &by_id, &mut listing.claimed_desktop_ids);
            listing.owned_paths.extend(files);
            if app.has_gui || cx.opts.include_all {
                listing.apps.push(app);
            }
        }
        // Packages not installed (`rc`, half-installed) still own paths.
        listing.owned_paths.extend(lists.into_values().flatten());
        listing.apps.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(listing)
    }
}

fn display(p: &Path) -> String {
    p.display().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::desktop::discover_desktop_entries;
    use crate::ownership::{load_dpkg, load_package_db};
    use crate::sources::{AppSource, ListOptions, SourceContext};
    use crate::testing::{FakeRunner, FixtureTree};
    use std::path::Path;

    const STATUS: &[u8] = include_bytes!("../../tests/fixtures/dpkg/status");
    const LISTS: [(&str, &[u8]); 4] = [
        (
            "coreutils.list",
            include_bytes!("../../tests/fixtures/dpkg/info/coreutils.list"),
        ),
        (
            "libacl1:amd64.list",
            include_bytes!("../../tests/fixtures/dpkg/info/libacl1:amd64.list"),
        ),
        (
            "firefox-esr.list",
            include_bytes!("../../tests/fixtures/dpkg/info/firefox-esr.list"),
        ),
        (
            "oldpkg.list",
            include_bytes!("../../tests/fixtures/dpkg/info/oldpkg.list"),
        ),
    ];
    const FIREFOX_DESKTOP: &str = "[Desktop Entry]\nType=Application\nName=Firefox ESR\nExec=/usr/lib/firefox-esr/firefox-esr %u\n";

    fn debian() -> FixtureTree {
        let t = FixtureTree::new();
        t.write("/var/lib/dpkg/status", STATUS);
        for (name, bytes) in LISTS {
            t.write(&format!("/var/lib/dpkg/info/{name}"), bytes);
        }
        t.write(
            "/usr/share/applications/firefox-esr.desktop",
            FIREFOX_DESKTOP.as_bytes(),
        );
        t
    }

    fn list(t: &FixtureTree, include_all: bool) -> SourceListing {
        let env = t.env();
        let desktop = discover_desktop_entries(&env);
        let runner = FakeRunner::new();
        let opts = ListOptions { include_all };
        let cx = SourceContext {
            env: &env,
            runner: &runner,
            desktop: &desktop,
            opts: &opts,
            config: &Config::default(),
        };
        DpkgSource.list(&cx).unwrap()
    }

    #[test]
    fn dpkg_lists_gui_packages_from_status() {
        let t = debian();
        let l = list(&t, false);
        let ids: Vec<&str> = l.apps.iter().map(|a| a.id.as_str()).collect();
        assert_eq!(ids, ["dpkg:firefox-esr"]);
        let ff = &l.apps[0];
        assert_eq!(ff.version.as_deref(), Some("140.4.0esr-1"));
        assert_eq!(ff.display_name, "Firefox ESR");
        assert_eq!(ff.desktop_ids, ["firefox-esr.desktop"]);
        assert!(l.claimed_desktop_ids.contains("firefox-esr.desktop"));
        let all = list(&t, true);
        let mut names: Vec<&str> = all.apps.iter().map(|a| a.name.as_str()).collect();
        names.sort();
        assert_eq!(names, ["coreutils", "firefox-esr", "libacl1"]);
        assert!(l.errors.is_empty(), "{:?}", l.errors);
    }

    #[test]
    fn dpkg_rc_packages_not_listed_but_owned() {
        let t = debian();
        let all = list(&t, true);
        assert!(!all.apps.iter().any(|a| a.name == "oldpkg"));
        assert!(
            all.owned_paths
                .contains(Path::new("/etc/oldpkg/oldpkg.conf"))
        );
    }

    #[test]
    fn dpkg_multiarch_list_files_are_read() {
        let t = debian();
        let all = list(&t, true);
        assert!(
            all.owned_paths
                .contains(Path::new("/usr/lib/x86_64-linux-gnu/libacl.so.1"))
        );
        assert!(!all.owned_paths.contains(Path::new("/.")));
    }

    #[test]
    fn dpkg_install_size_is_kib_times_1024() {
        let t = debian();
        let all = list(&t, true);
        let core = all.apps.iter().find(|a| a.name == "coreutils").unwrap();
        assert_eq!(core.install_size, Some(18457 * 1024));
    }

    #[test]
    fn dpkg_unreadable_list_makes_index_incomplete() {
        let t = debian();
        let list_path = t.env().sys("/var/lib/dpkg/info/coreutils.list");
        std::fs::remove_file(&list_path).unwrap();
        std::fs::create_dir(&list_path).unwrap();
        let l = list(&t, true);
        assert!(!l.errors.is_empty());
        assert!(load_dpkg(&t.env()).is_err());
    }

    #[test]
    fn load_package_db_on_debian_fixture_owns_usr_bin_ls() {
        let t = debian();
        let idx = load_package_db(&t.env(), &FakeRunner::new())
            .unwrap()
            .unwrap();
        assert!(idx.is_complete());
        assert!(idx.owns(Path::new("/usr/bin/cat")));
        assert!(idx.owns_at_or_below(Path::new("/usr/lib/firefox-esr")));
        assert!(!idx.owns_at_or_below(Path::new("/opt/inky")));
    }
}
