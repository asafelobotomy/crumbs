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
    /// The package is unpacked and configured: the third word of `Status`
    /// is `installed` (`install ok installed`, `hold ok installed`, ...).
    pub fn installed(&self) -> bool {
        self.status.split_whitespace().nth(2) == Some("installed")
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

pub(crate) const DIVERSIONS: &str = "/var/lib/dpkg/diversions";

/// The diverted-to paths of `/var/lib/dpkg/diversions` (triples of lines:
/// original, diverted-to, package). They belong to the package that
/// diverted them, though no `.list` names them. A missing file is fine.
fn diverted_paths(env: &Env) -> Result<Vec<PathBuf>, CoreError> {
    let path = env.sys(DIVERSIONS);
    let bytes = match fs::read(&path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(source) => return Err(CoreError::Io { path, source }),
    };
    Ok(bytes
        .split(|&b| b == b'\n')
        .skip(1)
        .step_by(3)
        .filter(|l| l.starts_with(b"/"))
        .map(|l| PathBuf::from(OsStr::from_bytes(l)))
        .collect())
}

/// The installed packages whose `.list` file is missing; each one makes
/// the owned index incomplete.
fn missing_lists(stanzas: &[Stanza], lists: &HashMap<String, Vec<PathBuf>>) -> Vec<String> {
    stanzas
        .iter()
        .filter(|s| s.installed() && !lists.contains_key(&s.name))
        .map(|s| s.name.clone())
        .collect()
}

/// Every path the dpkg database lists as owned, diversions included. Any
/// unreadable list, or an installed package without one, is an error, like
/// [`crate::ownership::load_pacman`].
pub(crate) fn owned_paths(env: &Env) -> Result<Vec<PathBuf>, CoreError> {
    let status_path = env.sys(STATUS);
    let status = fs::read(&status_path).map_err(|source| CoreError::Io {
        path: status_path.clone(),
        source,
    })?;
    let stanzas = parse_status(&String::from_utf8_lossy(&status));
    let (lists, errors) = read_lists(env);
    if let Some((path, source)) = errors.into_iter().next() {
        return Err(CoreError::Io { path, source });
    }
    if let Some(name) = missing_lists(&stanzas, &lists).into_iter().next() {
        return Err(CoreError::Parse {
            what: "dpkg database".into(),
            message: format!("installed package {name} has no file list"),
        });
    }
    let mut all: Vec<PathBuf> = lists.into_values().flatten().collect();
    all.extend(diverted_paths(env)?);
    Ok(all)
}

impl AppSource for DpkgSource {
    fn kind(&self) -> SourceKind {
        SourceKind::Dpkg
    }

    fn available(&self, env: &Env) -> bool {
        crate::ownership::dpkg_present(env)
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
        for name in missing_lists(&stanzas, &lists) {
            listing.errors.push(SourceError {
                source: SourceKind::Dpkg,
                message: format!("installed package {name} has no file list"),
            });
        }
        match diverted_paths(cx.env) {
            Ok(p) => listing.owned_paths.extend(p),
            Err(e) => listing.errors.push(SourceError {
                source: SourceKind::Dpkg,
                message: e.to_string(),
            }),
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
mod tests;
