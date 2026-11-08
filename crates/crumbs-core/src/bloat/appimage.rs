//! `appimage-old`: superseded AppImage versions.
//!
//! AppImages are grouped by normalized base name. Within a group the highest
//! version is kept, as is any version a launcher (menu or autostart entry)
//! runs and any version that is running now. A version that is missing or
//! doesn't parse is never old.

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use super::cache::new_item;
use super::{BloatItem, Category, ScanContext, Scanner};
use crate::desktop::{discover_desktop_entries, parse_desktop};
use crate::error::CoreError;
use crate::executor::running_processes;
use crate::model::{App, SourceKind};
use crate::sources::appimage::{normalize_name, split_appimage_name};

/// Compares two versions component by component, splitting on `.`, `-` and
/// `_`. Components that are both numbers compare numerically, others
/// lexically; a number against a tag is incomparable (`None`). A missing component is less than a numeric one, but greater
/// than a tag such as `beta`, so `2.0` is newer than `2.0-beta`. `None` when
/// either version is empty or the two are incomparable.
pub fn compare_versions(a: &str, b: &str) -> Option<Ordering> {
    if a.is_empty() || b.is_empty() {
        return None;
    }
    let split = |s: &str| -> Vec<String> {
        s.split(['.', '-', '_'])
            .filter(|c| !c.is_empty())
            .map(String::from)
            .collect()
    };
    let (a, b) = (split(a), split(b));
    let is_num = |c: &str| c.bytes().all(|b| b.is_ascii_digit());
    for i in 0..a.len().max(b.len()) {
        let ord = match (a.get(i), b.get(i)) {
            (Some(x), Some(y)) if is_num(x) && is_num(y) => {
                let (x, y) = (x.trim_start_matches('0'), y.trim_start_matches('0'));
                x.len().cmp(&y.len()).then_with(|| x.cmp(y))
            }
            // A number against a tag can't be ordered.
            (Some(x), Some(y)) if is_num(x) != is_num(y) => return None,
            (Some(x), Some(y)) => x.cmp(y),
            (Some(x), None) => {
                if is_num(x) {
                    Ordering::Greater
                } else {
                    Ordering::Less
                }
            }
            (None, Some(y)) => {
                if is_num(y) {
                    Ordering::Less
                } else {
                    Ordering::Greater
                }
            }
            (None, None) => Ordering::Equal,
        };
        if ord != Ordering::Equal {
            return Some(ord);
        }
    }
    Some(Ordering::Equal)
}

/// An AppImage in a group: app, file name, and its version if comparable.
type Member<'a> = (&'a App, String, Option<&'a str>);

/// In-system programs that menu or autostart entries execute, plus their
/// canonical (symlink-resolved) in-system forms.
fn launcher_targets(cx: &ScanContext) -> HashSet<PathBuf> {
    let home = cx.env.unsys(&cx.env.home);
    let mut programs: Vec<String> = discover_desktop_entries(cx.env)
        .into_iter()
        .filter_map(|e| e.exec_program)
        .collect();
    if let Ok(rd) = std::fs::read_dir(cx.env.config_home.join("autostart")) {
        for entry in rd.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if !name.ends_with(".desktop") {
                continue;
            }
            if let Ok(bytes) = std::fs::read(entry.path())
                && let Some(p) = parse_desktop(&name, &entry.path(), &bytes).exec_program
            {
                programs.push(p);
            }
        }
    }
    let mut targets = HashSet::new();
    for p in programs {
        let target = match p.strip_prefix("~/") {
            Some(rest) => home.join(rest),
            None => PathBuf::from(p),
        };
        if target.is_absolute()
            && let Ok(real) = cx.env.sys_path(&target).canonicalize()
        {
            targets.insert(cx.env.unsys(&real));
        }
        targets.insert(target);
    }
    targets
}

pub struct AppimageOldScanner;

impl Scanner for AppimageOldScanner {
    fn category(&self) -> Category {
        Category::AppimageOld
    }

    fn scan(&self, cx: &ScanContext) -> Result<Vec<BloatItem>, CoreError> {
        let mut groups: HashMap<String, Vec<Member>> = HashMap::new();
        for app in cx
            .inv
            .apps
            .iter()
            .filter(|a| a.source == SourceKind::AppImage)
        {
            let Some(file) = app
                .location
                .as_deref()
                .and_then(|l| l.file_name())
                .map(|n| n.to_string_lossy().into_owned())
            else {
                continue;
            };
            let (base, _) = split_appimage_name(&file);
            let version = app
                .version
                .as_deref()
                .filter(|v| compare_versions(v, v).is_some());
            groups
                .entry(normalize_name(&base))
                .or_default()
                .push((app, file, version));
        }

        let targets = launcher_targets(cx);
        let mut out = Vec::new();
        for members in groups.values() {
            for (app, file, version) in members {
                let Some(version) = version else { continue };
                // Old only when some comparable sibling is strictly newer.
                let superseded = members.iter().any(|(_, _, other)| {
                    other.and_then(|o| compare_versions(version, o)) == Some(Ordering::Less)
                });
                if !superseded {
                    continue;
                }
                let Some(location) = app.location.clone() else {
                    continue;
                };
                let canonical = cx
                    .env
                    .sys_path(&location)
                    .canonicalize()
                    .map(|p| cx.env.unsys(&p));
                if targets.contains(&location) || canonical.is_ok_and(|c| targets.contains(&c)) {
                    continue;
                }
                if !running_processes(cx.env, app).is_empty() {
                    continue;
                }
                if let Some(mut item) =
                    new_item(cx, Category::AppimageOld, location, file.clone(), false)
                {
                    item.app_id = Some(app.id.clone());
                    out.push(item);
                }
            }
        }
        out.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::inventory::Inventory;
    use crate::model::{App, SourceKind};
    use crate::safety::Scope;
    use crate::testing::{FakeRunner, FixtureTree};
    use std::path::PathBuf;
    use std::time::SystemTime;

    use super::super::ItemAction;

    #[test]
    fn compare_versions_table() {
        use Ordering::*;
        assert_eq!(compare_versions("0.19.0", "0.18.9"), Some(Greater));
        assert_eq!(compare_versions("1.10", "1.9"), Some(Greater));
        assert_eq!(compare_versions("2.0", "2.0-beta"), Some(Greater));
        assert_eq!(compare_versions("2.0-beta", "2.0"), Some(Less));
        assert_eq!(compare_versions("1.0", "1.0.1"), Some(Less));
        assert_eq!(compare_versions("1_2-3", "1.2.3"), Some(Equal));
        assert_eq!(compare_versions("1.0-alpha", "1.0-beta"), Some(Less));
        // A number against a tag is incomparable.
        assert_eq!(compare_versions("v1.2", "1.3"), None);
        assert_eq!(compare_versions("nightly", "1.0"), None);
        assert_eq!(compare_versions("", "1.0"), None);
        assert_eq!(compare_versions("1.0", ""), None);
    }

    fn app(t: &FixtureTree, dir: &str, file: &str) -> App {
        t.appimage(&format!("home/user/{dir}/{file}"));
        let mut a = App::new(SourceKind::AppImage, file);
        a.id = format!("appimage:/home/user/{dir}/{file}");
        a.display_name = file.into();
        let (_, v) = crate::sources::appimage::split_appimage_name(file);
        a.version = v;
        a.location = Some(PathBuf::from(format!("/home/user/{dir}/{file}")));
        a
    }

    fn scan(t: &FixtureTree, apps: Vec<App>) -> Vec<BloatItem> {
        let env = t.env();
        let cfg = Config::default();
        let inv = Inventory {
            apps,
            statuses: vec![],
            errors: vec![],
            owned: Default::default(),
        };
        let runner = FakeRunner::new();
        let cx = ScanContext {
            env: &env,
            cfg: &cfg,
            inv: &inv,
            runner: &runner,
            now: SystemTime::now(),
        };
        AppimageOldScanner.scan(&cx).unwrap()
    }

    fn labels(items: &[BloatItem]) -> Vec<&str> {
        items.iter().map(|i| i.label.as_str()).collect()
    }

    #[test]
    fn appimage_old_keeps_newest() {
        let t = FixtureTree::new();
        let apps = vec![
            app(&t, "Applications", "Foo_Bar-1.9-x86_64.AppImage"),
            app(&t, "Applications", "foo-bar-1.10.AppImage"),
            app(&t, "Downloads", "Foo_Bar-1.2.AppImage"),
            app(&t, "Applications", "Other-0.1.AppImage"),
        ];
        let items = scan(&t, apps);
        assert_eq!(
            labels(&items),
            ["Foo_Bar-1.9-x86_64.AppImage", "Foo_Bar-1.2.AppImage"]
        );
        let i = &items[1];
        assert_eq!(i.category, Category::AppimageOld);
        assert_eq!(
            i.path,
            Some("/home/user/Downloads/Foo_Bar-1.2.AppImage".into())
        );
        assert_eq!(i.action, ItemAction::Trash);
        assert_eq!(i.scope, Scope::Home);
        assert_eq!(
            i.app_id.as_deref(),
            Some("appimage:/home/user/Downloads/Foo_Bar-1.2.AppImage")
        );
        assert!(i.fingerprint.is_some() && i.preselected && i.note.is_none());
    }

    #[test]
    fn appimage_old_keeps_launcher_target() {
        let t = FixtureTree::new();
        let apps = vec![
            app(&t, "Applications", "Foo-1.0.AppImage"),
            app(&t, "Applications", "Foo-2.0.AppImage"),
            app(&t, "Applications", "Foo-0.5.AppImage"),
            app(&t, "Applications", "Foo-0.1.AppImage"),
        ];
        // Menu entry (with ~ expansion) pins 1.0; autostart pins 0.5.
        t.desktop(
            "home/user/.local/share/applications/foo.desktop",
            "[Desktop Entry]\nType=Application\nName=Foo\nExec=~/Applications/Foo-1.0.AppImage %U\n",
        );
        t.desktop(
            "home/user/.config/autostart/foo.desktop",
            "[Desktop Entry]\nType=Application\nName=Foo\nExec=/home/user/Applications/Foo-0.5.AppImage\n",
        );
        let items = scan(&t, apps);
        assert_eq!(labels(&items), ["Foo-0.1.AppImage"]);
    }

    #[test]
    fn appimage_old_keeps_running() {
        let t = FixtureTree::new();
        let mut running = app(&t, "Applications", "Foo-1.0.AppImage");
        running.exec_names = vec!["Foo-1.0.AppImage".into()];
        let apps = vec![
            running,
            app(&t, "Applications", "Foo-0.5.AppImage"),
            app(&t, "Applications", "Foo-2.0.AppImage"),
        ];
        t.symlink("proc/42/exe", "/tmp/.mount_Foo/foo");
        t.write(
            "proc/42/environ",
            b"HOME=/home/user\0APPIMAGE=/home/user/Applications/Foo-1.0.AppImage\0",
        );
        let items = scan(&t, apps);
        assert_eq!(labels(&items), ["Foo-0.5.AppImage"]);
    }

    #[test]
    fn unparseable_version_never_old() {
        let t = FixtureTree::new();
        let apps = vec![
            app(&t, "Applications", "Foo-x86_64.AppImage"),
            app(&t, "Applications", "Foo-1.0.AppImage"),
            app(&t, "Applications", "Foo-2.0.AppImage"),
        ];
        let items = scan(&t, apps);
        assert_eq!(labels(&items), ["Foo-1.0.AppImage"]);
        // A group of only unversioned files has no old member.
        let t2 = FixtureTree::new();
        let items = scan(
            &t2,
            vec![
                app(&t2, "Applications", "Bar.AppImage"),
                app(&t2, "Downloads", "Bar.AppImage"),
            ],
        );
        assert!(items.is_empty());
    }

    #[test]
    fn incomparable_versions_are_never_old() {
        let t = FixtureTree::new();
        let mut nightly = app(&t, "Applications", "Foo-nightly.AppImage");
        nightly.version = Some("nightly".into());
        let items = scan(
            &t,
            vec![
                nightly,
                app(&t, "Applications", "Foo-1.0.AppImage"),
                app(&t, "Applications", "Foo-2.0.AppImage"),
            ],
        );
        assert_eq!(labels(&items), ["Foo-1.0.AppImage"]);
        // Same with the tag-bearing app listed last.
        let t2 = FixtureTree::new();
        let mut v = app(&t2, "Applications", "Foo-v1.2.AppImage");
        v.version = Some("v1.2".into());
        let items = scan(&t2, vec![app(&t2, "Applications", "Foo-1.3.AppImage"), v]);
        assert!(items.is_empty(), "{items:?}");
    }

    #[test]
    fn launcher_through_symlinked_file_keeps_target() {
        let t = FixtureTree::new();
        let apps = vec![
            app(&t, "Applications", "Foo-1.0.AppImage"),
            app(&t, "Applications", "Foo-2.0.AppImage"),
            app(&t, "Applications", "Foo-0.1.AppImage"),
        ];
        // Relative, so the link stays inside the fake root.
        t.symlink("home/user/Applications/Foo.AppImage", "Foo-1.0.AppImage");
        t.desktop(
            "home/user/.local/share/applications/foo.desktop",
            "[Desktop Entry]\nType=Application\nName=Foo\nExec=~/Applications/Foo.AppImage\n",
        );
        let items = scan(&t, apps);
        assert_eq!(labels(&items), ["Foo-0.1.AppImage"]);
    }

    #[test]
    fn launcher_through_symlinked_dir_keeps_target() {
        let t = FixtureTree::new();
        let apps = vec![
            app(&t, "Applications", "Foo-1.0.AppImage"),
            app(&t, "Applications", "Foo-2.0.AppImage"),
            app(&t, "Applications", "Foo-0.1.AppImage"),
        ];
        t.symlink("home/user/apps", "Applications");
        t.desktop(
            "home/user/.config/autostart/foo.desktop",
            "[Desktop Entry]\nType=Application\nName=Foo\nExec=/home/user/apps/Foo-1.0.AppImage\n",
        );
        let items = scan(&t, apps);
        assert_eq!(labels(&items), ["Foo-0.1.AppImage"]);
    }

    #[test]
    fn names_differing_only_in_case_or_separators_share_a_group() {
        // Grouping uses normalize_name, so Tool-1.0 and tool-2.0 are one app.
        let t = FixtureTree::new();
        let items = scan(
            &t,
            vec![
                app(&t, "Applications", "Tool-1.0.AppImage"),
                app(&t, "Applications", "tool-2.0.AppImage"),
            ],
        );
        assert_eq!(labels(&items), ["Tool-1.0.AppImage"]);
    }

    #[test]
    fn equal_max_versions_in_two_dirs_are_both_kept() {
        let t = FixtureTree::new();
        let items = scan(
            &t,
            vec![
                app(&t, "Applications", "Foo-2.0.AppImage"),
                app(&t, "Downloads", "Foo-2.0.AppImage"),
                app(&t, "Downloads", "Foo-1.0.AppImage"),
            ],
        );
        assert_eq!(labels(&items), ["Foo-1.0.AppImage"]);
    }
}
