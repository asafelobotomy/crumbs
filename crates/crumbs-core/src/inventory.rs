//! Whole-system app discovery across all sources.

use std::collections::HashSet;

use serde::Serialize;

use crate::command::CommandRunner;
use crate::desktop::discover_desktop_entries;
use crate::env::Env;
use crate::error::SourceError;
use crate::model::{App, SourceKind};
use crate::sources::appimage::{AppImageSource, normalize_name};
use crate::sources::flatpak::FlatpakSource;
use crate::sources::pacman::PacmanSource;
use crate::sources::unmanaged::list_unmanaged;
use crate::sources::{AppSource, ListOptions, SourceContext};

#[derive(Serialize, Debug, Clone, PartialEq)]
pub struct SourceStatus {
    pub source: SourceKind,
    pub available: bool,
    pub app_count: usize,
    pub skipped_reason: Option<String>,
}

#[derive(Serialize, Debug, Clone, PartialEq)]
pub struct Inventory {
    pub apps: Vec<App>,
    pub statuses: Vec<SourceStatus>,
    pub errors: Vec<SourceError>,
}

pub fn default_sources() -> Vec<Box<dyn AppSource>> {
    vec![
        Box::new(PacmanSource),
        Box::new(FlatpakSource),
        Box::new(AppImageSource),
    ]
}

fn unavailable_reason(kind: SourceKind) -> &'static str {
    match kind {
        SourceKind::Pacman => "no pacman database",
        SourceKind::Flatpak => "not installed",
        _ => "not available",
    }
}

/// Lists apps from every available source, then unmanaged apps when a
/// package database was read successfully.
pub fn discover(env: &Env, runner: &dyn CommandRunner, opts: &ListOptions) -> Inventory {
    let desktop = discover_desktop_entries(env);
    let cx = SourceContext {
        env,
        runner,
        desktop: &desktop,
        opts,
    };

    let mut apps = Vec::new();
    let mut statuses = Vec::new();
    let mut errors = Vec::new();
    let mut owned = HashSet::new();
    let mut claimed = HashSet::new();
    let mut package_db_listed = false;
    let mut package_db_incomplete = false;

    for source in default_sources() {
        let kind = source.kind();
        if !source.available(env) {
            statuses.push(SourceStatus {
                source: kind,
                available: false,
                app_count: 0,
                skipped_reason: Some(unavailable_reason(kind).into()),
            });
            continue;
        }
        match source.list(&cx) {
            Ok(listing) => {
                if source.is_package_db() {
                    if listing.errors.is_empty() {
                        package_db_listed = true;
                    } else {
                        package_db_incomplete = true;
                    }
                }
                statuses.push(SourceStatus {
                    source: kind,
                    available: true,
                    app_count: listing.apps.len(),
                    skipped_reason: None,
                });
                owned.extend(listing.owned_paths);
                claimed.extend(listing.claimed_desktop_ids);
                errors.extend(listing.errors);
                apps.extend(listing.apps);
            }
            Err(e) => {
                statuses.push(SourceStatus {
                    source: kind,
                    available: true,
                    app_count: 0,
                    skipped_reason: None,
                });
                if source.is_package_db() {
                    package_db_incomplete = true;
                }
                errors.push(SourceError {
                    source: kind,
                    message: e.to_string(),
                });
            }
        }
    }
    // An incomplete ownership set must never be used to classify unmanaged apps.
    package_db_listed &= !package_db_incomplete;

    if package_db_listed {
        let listing = list_unmanaged(&cx, &owned, &claimed);
        statuses.push(SourceStatus {
            source: SourceKind::Unmanaged,
            available: true,
            app_count: listing.apps.len(),
            skipped_reason: None,
        });
        apps.extend(listing.apps);
    } else {
        statuses.push(SourceStatus {
            source: SourceKind::Unmanaged,
            available: false,
            app_count: 0,
            skipped_reason: Some(
                if package_db_incomplete {
                    "package database incomplete"
                } else {
                    "no supported package database found"
                }
                .into(),
            ),
        });
    }

    apps.sort_by(|a, b| {
        a.display_name
            .to_lowercase()
            .cmp(&b.display_name.to_lowercase())
            .then_with(|| a.id.cmp(&b.id))
    });
    Inventory {
        apps,
        statuses,
        errors,
    }
}

#[derive(Debug)]
pub enum Resolved<'a> {
    Found(&'a App),
    Ambiguous(Vec<&'a App>),
    NotFound,
}

/// Finds an app by id, name, desktop ID, rDNS ID, display name or loose name.
/// The first matching rule decides; several matches in it are ambiguous.
pub fn find_app<'a>(inv: &'a Inventory, query: &str) -> Resolved<'a> {
    let with_ext = if query.ends_with(".desktop") {
        query.to_string()
    } else {
        format!("{query}.desktop")
    };
    let lower = query.to_lowercase();
    let norm = normalize_name(query);

    let rules: [&dyn Fn(&App) -> bool; 6] = [
        &|a| a.id == query,
        &|a| a.name == query,
        &|a| a.desktop_ids.iter().any(|d| d == query || *d == with_ext),
        &|a| a.rdns_id.as_deref() == Some(query),
        &|a| a.display_name.to_lowercase() == lower,
        &|a| {
            !norm.is_empty()
                && (normalize_name(&a.name) == norm || normalize_name(&a.display_name) == norm)
        },
    ];
    for rule in rules {
        let mut hits: Vec<&App> = inv.apps.iter().filter(|a| rule(a)).collect();
        match hits.len() {
            0 => continue,
            1 => return Resolved::Found(hits.remove(0)),
            _ => return Resolved::Ambiguous(hits),
        }
    }
    Resolved::NotFound
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::CmdOutput;
    use crate::testing::{FakeRunner, FixtureTree};

    const FLATPAK_ARGS: [&str; 3] = [
        "list",
        "--app",
        "--columns=application,name,version,installation,size",
    ];

    fn ok(stdout: &str) -> CmdOutput {
        CmdOutput {
            status: 0,
            stdout: stdout.into(),
            stderr: String::new(),
        }
    }

    fn pacman_db(t: &FixtureTree) {
        t.mkdir("/var/lib/pacman/local");
    }

    fn firefox(t: &FixtureTree) {
        t.pacman_pkg(
            "firefox",
            "159.0.1-1",
            None,
            &[
                "usr/bin/firefox",
                "usr/share/applications/",
                "usr/share/applications/firefox.desktop",
            ],
        );
        t.desktop(
            "/usr/share/applications/firefox.desktop",
            "[Desktop Entry]\nType=Application\nName=Firefox Web Browser\nExec=/usr/bin/firefox %u\n",
        );
        t.write_exec("/usr/bin/firefox", b"");
    }

    fn app(source: SourceKind, name: &str, display: &str) -> App {
        let mut a = App::new(source, name);
        a.display_name = display.into();
        a
    }

    fn inv(apps: Vec<App>) -> Inventory {
        Inventory {
            apps,
            statuses: Vec::new(),
            errors: Vec::new(),
        }
    }

    fn found<'a>(r: Resolved<'a>) -> &'a App {
        match r {
            Resolved::Found(a) => a,
            other => panic!("expected Found, got {other:?}"),
        }
    }

    #[test]
    fn unmanaged_via_discover_and_status_order() {
        let t = FixtureTree::new();
        firefox(&t);
        t.write_exec("/opt/foo/foo", b"");
        t.desktop(
            "/usr/share/applications/foo.desktop",
            "[Desktop Entry]\nType=Application\nName=Zed Foo\nExec=/opt/foo/foo\n",
        );
        t.appimage("/home/user/Applications/Tool-1.0-x86_64.AppImage");
        let inv = discover(&t.env(), &FakeRunner::new(), &ListOptions::default());
        let kinds: Vec<_> = inv.statuses.iter().map(|s| s.source).collect();
        assert_eq!(
            kinds,
            [
                SourceKind::Pacman,
                SourceKind::Flatpak,
                SourceKind::AppImage,
                SourceKind::Unmanaged
            ]
        );
        let counts: Vec<_> = inv.statuses.iter().map(|s| s.app_count).collect();
        assert_eq!(counts, [1, 0, 1, 1]);
        assert_eq!(
            inv.statuses[1].skipped_reason.as_deref(),
            Some("not installed")
        );
        assert!(!inv.statuses[1].available);
        let ids: Vec<_> = inv.apps.iter().map(|a| a.id.as_str()).collect();
        assert_eq!(
            ids,
            [
                "pacman:firefox",
                "appimage:~/Applications/Tool-1.0-x86_64.AppImage",
                "unmanaged:foo.desktop"
            ]
        );
        assert!(inv.errors.is_empty());
    }

    #[test]
    fn unmanaged_skipped_when_package_db_has_errors() {
        let t = FixtureTree::new();
        firefox(&t);
        t.write(
            "/var/lib/pacman/local/nofiles-1-1/desc",
            b"%NAME%\nnofiles\n\n%VERSION%\n1-1\n\n",
        );
        t.write_exec("/opt/foo/foo", b"");
        t.desktop(
            "/usr/share/applications/foo.desktop",
            "[Desktop Entry]\nType=Application\nName=Foo\nExec=/opt/foo/foo\n",
        );
        let inv = discover(&t.env(), &FakeRunner::new(), &ListOptions::default());
        assert!(!inv.errors.is_empty());
        let st = inv.statuses.last().unwrap();
        assert_eq!(st.source, SourceKind::Unmanaged);
        assert!(!st.available);
        assert_eq!(
            st.skipped_reason.as_deref(),
            Some("package database incomplete")
        );
        assert!(inv.apps.iter().all(|a| a.source != SourceKind::Unmanaged));
    }

    #[test]
    fn symlinked_exec_links_appimage_and_is_not_unmanaged() {
        let t = FixtureTree::new();
        pacman_db(&t);
        t.appimage("/home/user/Applications/Foo-1.0-x86_64.AppImage");
        t.symlink(
            "/home/user/.local/share/foo/appimage",
            "../../../Applications/Foo-1.0-x86_64.AppImage",
        );
        t.desktop(
            "/home/user/.local/share/applications/foo.desktop",
            "[Desktop Entry]\nType=Application\nName=Foo\nExec=~/.local/share/foo/appimage\n",
        );
        let inv = discover(&t.env(), &FakeRunner::new(), &ListOptions::default());
        let ids: Vec<_> = inv.apps.iter().map(|a| a.id.as_str()).collect();
        assert_eq!(ids, ["appimage:~/Applications/Foo-1.0-x86_64.AppImage"]);
        assert_eq!(inv.apps[0].desktop_ids, ["foo.desktop"]);
    }

    #[test]
    fn unmanaged_skipped_without_package_db() {
        let t = FixtureTree::new();
        t.write_exec("/opt/foo/foo", b"");
        t.desktop(
            "/usr/share/applications/foo.desktop",
            "[Desktop Entry]\nType=Application\nName=Foo\nExec=/opt/foo/foo\n",
        );
        let inv = discover(&t.env(), &FakeRunner::new(), &ListOptions::default());
        let st = inv.statuses.last().unwrap();
        assert_eq!(st.source, SourceKind::Unmanaged);
        assert_eq!(
            st.skipped_reason.as_deref(),
            Some("no supported package database found")
        );
        assert_eq!(st.app_count, 0);
        assert!(inv.apps.iter().all(|a| a.source != SourceKind::Unmanaged));
        assert_eq!(
            inv.statuses[0].skipped_reason.as_deref(),
            Some("no pacman database")
        );
    }

    #[test]
    fn failing_source_does_not_stop_others() {
        let t = FixtureTree::new();
        pacman_db(&t);
        firefox(&t);
        t.write_exec("/usr/bin/flatpak", b"");
        t.appimage("/home/user/Applications/Tool-1.0-x86_64.AppImage");
        let inv = discover(&t.env(), &FakeRunner::new(), &ListOptions::default());
        assert_eq!(inv.errors.len(), 1);
        assert_eq!(inv.errors[0].source, SourceKind::Flatpak);
        assert!(inv.apps.iter().any(|a| a.source == SourceKind::Pacman));
        assert!(inv.apps.iter().any(|a| a.source == SourceKind::AppImage));
        assert!(inv.statuses[1].available);
    }

    #[test]
    fn flatpak_claims_its_desktop_entry() {
        let t = FixtureTree::new();
        pacman_db(&t);
        t.write_exec("/usr/bin/flatpak", b"");
        t.write_exec("/usr/bin/flatpak-wrapper", b"");
        t.desktop(
            "/usr/share/applications/org.x.Y.desktop",
            "[Desktop Entry]\nType=Application\nName=Y\nExec=/usr/bin/flatpak-wrapper\n",
        );
        let runner =
            FakeRunner::new().on("flatpak", &FLATPAK_ARGS, ok("org.x.Y\tY\t1\tuser\t1 MB\n"));
        let inv = discover(&t.env(), &runner, &ListOptions::default());
        assert_eq!(inv.apps.len(), 1);
        assert_eq!(inv.apps[0].source, SourceKind::Flatpak);
    }

    #[test]
    fn find_app_precedence() {
        let t = FixtureTree::new();
        firefox(&t);
        t.write_exec("/usr/bin/flatpak", b"");
        let runner = FakeRunner::new().on(
            "flatpak",
            &FLATPAK_ARGS,
            ok("org.mozilla.firefox\tFirefox\t1\tsystem\t1 MB\n"),
        );
        let inv = discover(&t.env(), &runner, &ListOptions::default());
        assert_eq!(inv.apps.len(), 2);
        assert_eq!(found(find_app(&inv, "firefox")).id, "pacman:firefox");
        assert_eq!(
            found(find_app(&inv, "org.mozilla.firefox")).id,
            "flatpak:system/org.mozilla.firefox"
        );
        assert_eq!(
            found(find_app(&inv, "org.mozilla.firefox.desktop")).source,
            SourceKind::Flatpak
        );
        assert_eq!(
            found(find_app(&inv, "firefox.desktop")).id,
            "pacman:firefox"
        );
        assert_eq!(
            found(find_app(&inv, "FIREFOX WEB BROWSER")).id,
            "pacman:firefox"
        );
        assert!(matches!(find_app(&inv, "nope"), Resolved::NotFound));
    }

    #[test]
    fn find_app_rdns_and_normalized() {
        let mut fp = app(SourceKind::Flatpak, "org.x.Y", "Why");
        fp.rdns_id = Some("org.x.Y".into());
        let tool = app(SourceKind::AppImage, "Tool.AppImage", "My-Tool");
        let inv = inv(vec![fp, tool]);
        assert_eq!(
            found(find_app(&inv, "my tool")).id,
            "appimage:Tool.AppImage"
        );
        assert_eq!(found(find_app(&inv, "mytool")).id, "appimage:Tool.AppImage");
        assert_eq!(found(find_app(&inv, "org.x.y")).id, "flatpak:org.x.Y");
    }

    #[test]
    fn find_app_ambiguous() {
        let inv = inv(vec![
            app(SourceKind::Pacman, "firefox-a", "Firefox"),
            app(SourceKind::Flatpak, "org.mozilla.firefox", "Firefox"),
        ]);
        match find_app(&inv, "Firefox") {
            Resolved::Ambiguous(v) => assert_eq!(v.len(), 2),
            other => panic!("expected Ambiguous, got {other:?}"),
        }
    }

    #[test]
    fn inventory_serializes() {
        let t = FixtureTree::new();
        let inv = discover(&t.env(), &FakeRunner::new(), &ListOptions::default());
        let v = serde_json::to_value(&inv).unwrap();
        assert_eq!(v["statuses"][3]["source"], "unmanaged");
        assert!(v["apps"].is_array());
    }
}
