use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use crate::desktop::{DesktopEntry, exec_basename};
use crate::env::Env;
use crate::error::{CoreError, SourceError};
use crate::model::{App, SourceKind};

use super::{AppSource, SourceContext, SourceListing};

pub(crate) const DB_DIR: &str = "/var/lib/pacman/local";
const APPLICATIONS_DIR: &str = "/usr/share/applications/";

pub struct PacmanSource;

impl AppSource for PacmanSource {
    fn kind(&self) -> SourceKind {
        SourceKind::Pacman
    }

    fn available(&self, env: &Env) -> bool {
        env.sys(DB_DIR).is_dir()
    }

    fn is_package_db(&self) -> bool {
        true
    }

    fn list(&self, cx: &SourceContext) -> Result<SourceListing, CoreError> {
        let db = cx.env.sys(DB_DIR);
        let io_err = |path: &PathBuf, source| CoreError::Io {
            path: path.clone(),
            source,
        };
        let mut listing = SourceListing::default();
        let mut dirs: Vec<PathBuf> = Vec::new();
        for entry in fs::read_dir(&db).map_err(|e| io_err(&db, e))? {
            match entry {
                Ok(e) => {
                    let p = e.path();
                    if p.is_dir() {
                        dirs.push(p);
                    }
                }
                Err(e) => listing.errors.push(SourceError {
                    source: SourceKind::Pacman,
                    message: format!("cannot read package database entry: {e}"),
                }),
            }
        }
        dirs.sort();

        let by_id: HashMap<&str, &DesktopEntry> =
            cx.desktop.iter().map(|d| (d.id.as_str(), d)).collect();

        for dir in dirs {
            // Ownership must be complete even when desc is unusable.
            let files = match fs::read(dir.join("files")) {
                Ok(b) => owned_entries(&String::from_utf8_lossy(&b)),
                Err(e) => {
                    listing
                        .errors
                        .push(pkg_error(&dir, format!("cannot read files: {e}")));
                    (Vec::new(), Vec::new())
                }
            };
            let (files, pkg_dirs) = files;
            listing
                .owned_dirs
                .extend(pkg_dirs.into_iter().map(PathBuf::from));
            let desc_path = dir.join("desc");
            let desc = match fs::read(&desc_path) {
                Ok(b) => parse_sections(&String::from_utf8_lossy(&b)),
                Err(e) => {
                    listing
                        .errors
                        .push(pkg_error(&dir, format!("cannot read desc: {e}")));
                    listing
                        .owned_paths
                        .extend(files.into_iter().map(PathBuf::from));
                    continue;
                }
            };
            let Some(name) = first(&desc, "NAME") else {
                listing
                    .errors
                    .push(pkg_error(&dir, "desc has no %NAME%".into()));
                listing
                    .owned_paths
                    .extend(files.into_iter().map(PathBuf::from));
                continue;
            };

            let mut app = App::new(SourceKind::Pacman, &name);
            app.version = first(&desc, "VERSION");
            app.description = first(&desc, "DESC");
            app.install_size = first(&desc, "SIZE").and_then(|s| s.trim().parse().ok());

            let mut linked: Vec<&DesktopEntry> = Vec::new();
            for f in &files {
                let Some(rel) = f.strip_prefix(APPLICATIONS_DIR) else {
                    continue;
                };
                if !rel.ends_with(".desktop") {
                    continue;
                }
                // Nested directories map to dashes in desktop IDs.
                let id = rel.replace('/', "-");
                if let Some(entry) = by_id.get(id.as_str()) {
                    linked.push(entry);
                    listing.claimed_desktop_ids.insert(id);
                }
            }

            for e in &linked {
                app.desktop_ids.push(e.id.clone());
                app.exec_names.extend(exec_basename(e));
                app.wm_classes.extend(e.wm_class.clone());
            }
            app.has_gui = linked
                .iter()
                .any(|e| e.is_application && !e.no_display && !e.hidden);
            if let Some(n) = linked.first().and_then(|e| e.name.clone()) {
                app.display_name = n;
            }

            listing
                .owned_paths
                .extend(files.into_iter().map(PathBuf::from));
            if app.has_gui || cx.opts.include_all {
                listing.apps.push(app);
            }
        }
        Ok(listing)
    }
}

fn pkg_error(dir: &std::path::Path, message: String) -> SourceError {
    let pkg = dir
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    SourceError {
        source: SourceKind::Pacman,
        message: format!("{pkg}: {message}"),
    }
}

/// Splits a pacman db file into `%HEADER%` sections; each value is the lines
/// up to the next blank line.
pub(crate) fn parse_sections(text: &str) -> HashMap<String, Vec<String>> {
    let mut out = HashMap::new();
    let mut lines = text.lines();
    while let Some(line) = lines.next() {
        let Some(header) = line.strip_prefix('%').and_then(|h| h.strip_suffix('%')) else {
            continue;
        };
        let values: Vec<String> = lines
            .by_ref()
            .take_while(|l| !l.is_empty())
            .map(String::from)
            .collect();
        out.insert(header.to_string(), values);
    }
    out
}

pub(crate) fn first(sections: &HashMap<String, Vec<String>>, key: &str) -> Option<String> {
    sections
        .get(key)?
        .first()
        .cloned()
        .filter(|v| !v.is_empty())
}

/// In-system absolute `(files, directories)` from a `files` listing.
/// Directory lines lose their trailing `/`.
pub(crate) fn owned_entries(text: &str) -> (Vec<String>, Vec<String>) {
    let mut files = Vec::new();
    let mut dirs = Vec::new();
    for l in parse_sections(text).remove("FILES").unwrap_or_default() {
        match l.strip_suffix('/') {
            Some(d) => dirs.push(format!("/{d}")),
            None => files.push(format!("/{l}")),
        }
    }
    (files, dirs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::desktop::discover_desktop_entries;
    use crate::sources::ListOptions;
    use crate::testing::{FakeRunner, FixtureTree};
    use std::path::PathBuf;

    const FIREFOX_DESKTOP: &str = "[Desktop Entry]\nType=Application\nName=Firefox Web Browser\nExec=/usr/lib/firefox/firefox %u\nStartupWMClass=firefox\n";

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
        PacmanSource.list(&cx).unwrap()
    }

    fn firefox(t: &FixtureTree) {
        t.pacman_pkg(
            "firefox",
            "159.0.1-1",
            Some(316982060),
            &[
                "usr/",
                "usr/bin/",
                "usr/bin/firefox",
                "usr/share/applications/",
                "usr/share/applications/firefox.desktop",
            ],
        );
        t.desktop("/usr/share/applications/firefox.desktop", FIREFOX_DESKTOP);
    }

    #[test]
    fn lists_gui_package() {
        let t = FixtureTree::new();
        firefox(&t);
        let l = list(&t, false);
        assert_eq!(l.apps.len(), 1);
        let a = &l.apps[0];
        assert_eq!(a.id, "pacman:firefox");
        assert_eq!(a.display_name, "Firefox Web Browser");
        assert_eq!(a.version.as_deref(), Some("159.0.1-1"));
        assert_eq!(a.install_size, Some(316982060));
        assert_eq!(a.exec_names, ["firefox"]);
        assert_eq!(a.desktop_ids, ["firefox.desktop"]);
        assert_eq!(a.wm_classes, ["firefox"]);
        assert!(a.has_gui);
        assert!(l.claimed_desktop_ids.contains("firefox.desktop"));
        assert!(l.errors.is_empty());
    }

    #[test]
    fn hides_cli_package_unless_all() {
        let t = FixtureTree::new();
        firefox(&t);
        t.pacman_pkg("coreutils", "9.5-1", Some(1), &["usr/bin/ls"]);
        assert!(
            list(&t, false)
                .apps
                .iter()
                .all(|a| a.id != "pacman:coreutils")
        );
        let all = list(&t, true);
        let c = all
            .apps
            .iter()
            .find(|a| a.id == "pacman:coreutils")
            .unwrap();
        assert!(!c.has_gui);
        assert_eq!(c.display_name, "coreutils");
    }

    #[test]
    fn no_display_entry_is_not_gui() {
        let t = FixtureTree::new();
        t.pacman_pkg(
            "helper",
            "1-1",
            None,
            &["usr/share/applications/helper.desktop"],
        );
        t.desktop(
            "/usr/share/applications/helper.desktop",
            "[Desktop Entry]\nType=Application\nName=Helper\nExec=helper\nNoDisplay=true\n",
        );
        assert!(list(&t, false).apps.is_empty());
        let all = list(&t, true);
        assert_eq!(all.apps.len(), 1);
        assert!(!all.apps[0].has_gui);
    }

    #[test]
    fn owned_paths_cover_all_packages() {
        let t = FixtureTree::new();
        firefox(&t);
        t.pacman_pkg("coreutils", "9.5-1", Some(1), &["usr/", "usr/bin/ls"]);
        let l = list(&t, false);
        assert!(l.owned_paths.contains(&PathBuf::from("/usr/bin/ls")));
        assert!(l.owned_paths.contains(&PathBuf::from("/usr/bin/firefox")));
        assert!(!l.owned_paths.contains(&PathBuf::from("/usr/")));
        assert!(!l.owned_paths.contains(&PathBuf::from("/usr")));
        assert!(l.owned_dirs.contains(&PathBuf::from("/usr")));
        assert!(l.owned_dirs.contains(&PathBuf::from("/usr/bin")));
    }

    #[test]
    fn malformed_entry_recorded_not_fatal() {
        let t = FixtureTree::new();
        firefox(&t);
        t.write(
            "/var/lib/pacman/local/broken-1-1/files",
            b"%FILES%\nusr/x\n",
        );
        t.pacman_pkg("nosize", "2-1", None, &["usr/bin/nosize"]);
        t.write(
            "/var/lib/pacman/local/badbytes-1-1/desc",
            b"%NAME%\nbadbytes\n\n%VERSION%\n1-1\n\n",
        );
        t.write(
            "/var/lib/pacman/local/badbytes-1-1/files",
            b"%FILES%\nusr/bin/ok\nusr/bin/\xff\xfe\n",
        );
        let l = list(&t, true);
        assert_eq!(l.errors.len(), 1);
        assert_eq!(l.errors[0].source, SourceKind::Pacman);
        let ns = l.apps.iter().find(|a| a.id == "pacman:nosize").unwrap();
        assert_eq!(ns.install_size, None);
        assert!(l.apps.iter().any(|a| a.id == "pacman:badbytes"));
        assert!(l.owned_paths.contains(&PathBuf::from("/usr/bin/ok")));
        // Files of a package with an unreadable desc still count as owned.
        assert!(l.owned_paths.contains(&PathBuf::from("/usr/x")));
    }

    #[test]
    fn missing_files_is_an_error() {
        let t = FixtureTree::new();
        t.write(
            "/var/lib/pacman/local/nofiles-1-1/desc",
            b"%NAME%\nnofiles\n\n%VERSION%\n1-1\n\n",
        );
        let l = list(&t, true);
        assert_eq!(l.errors.len(), 1);
        assert!(l.errors[0].message.contains("nofiles-1-1"));
        assert!(l.errors[0].message.contains("cannot read files"));
        assert!(l.apps.iter().all(|a| !a.has_gui));
    }

    #[test]
    fn unreadable_desc_still_records_owned_files() {
        let t = FixtureTree::new();
        t.write(
            "/var/lib/pacman/local/nodesc-1-1/files",
            b"%FILES%\nopt/x/x\n",
        );
        let l = list(&t, true);
        assert_eq!(l.errors.len(), 1);
        assert!(l.owned_paths.contains(&PathBuf::from("/opt/x/x")));
    }

    #[test]
    fn unavailable_without_db() {
        let t = FixtureTree::new();
        assert!(!PacmanSource.available(&t.env()));
        t.mkdir("/var/lib/pacman/local");
        assert!(PacmanSource.available(&t.env()));
        assert!(PacmanSource.is_package_db());
    }
}
