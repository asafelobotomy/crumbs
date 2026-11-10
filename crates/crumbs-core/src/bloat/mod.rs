//! The bloat model: categories of reclaimable space, the scanner registry
//! and the parallel scan driver.

mod app;
mod appimage;
mod aur;
mod browser;
mod build;
mod cache;
mod containers;
mod dev;
mod flatpak;
mod logs;
#[cfg(test)]
#[path = "tests.rs"]
mod scanner_tests;
mod system;
pub(crate) mod temp;
mod trash;

use std::path::PathBuf;
use std::str::FromStr;
use std::time::SystemTime;

use rayon::prelude::*;
use serde::ser::SerializeSeq;
use serde::{Deserialize, Serialize, Serializer};

use crate::command::CommandRunner;
use crate::config::Config;
use crate::env::Env;
use crate::error::CoreError;
use crate::inventory::Inventory;
use crate::safety::{Fingerprint, Scope};
use crate::size::disk_usage_many;

/// A kind of reclaimable space. Declaration order is the display order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Category {
    Cache,
    Thumbnails,
    Trash,
    Temp,
    BrowserCache,
    DevCache,
    FlatpakUnused,
    AppimageOld,
    Logs,
    AurBuild,
    AppCache,
    Containers,
    BuildOutput,
    PkgCache,
    OrphanPkgs,
    Journal,
    Coredumps,
    SystemTemp,
    FlatpakUnusedSystem,
    Snapshots,
}

impl Category {
    pub const ALL: [Category; 20] = [
        Category::Cache,
        Category::Thumbnails,
        Category::Trash,
        Category::Temp,
        Category::BrowserCache,
        Category::DevCache,
        Category::FlatpakUnused,
        Category::AppimageOld,
        Category::Logs,
        Category::AurBuild,
        Category::AppCache,
        Category::Containers,
        Category::BuildOutput,
        Category::PkgCache,
        Category::OrphanPkgs,
        Category::Journal,
        Category::Coredumps,
        Category::SystemTemp,
        Category::FlatpakUnusedSystem,
        Category::Snapshots,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Category::Cache => "cache",
            Category::Thumbnails => "thumbnails",
            Category::Trash => "trash",
            Category::Temp => "temp",
            Category::BrowserCache => "browser-cache",
            Category::DevCache => "dev-cache",
            Category::FlatpakUnused => "flatpak-unused",
            Category::AppimageOld => "appimage-old",
            Category::Logs => "logs",
            Category::AurBuild => "aur-build",
            Category::AppCache => "app-cache",
            Category::Containers => "containers",
            Category::BuildOutput => "build-output",
            Category::PkgCache => "pkg-cache",
            Category::OrphanPkgs => "orphan-pkgs",
            Category::Journal => "journal",
            Category::Coredumps => "coredumps",
            Category::SystemTemp => "system-temp",
            Category::FlatpakUnusedSystem => "flatpak-unused-system",
            Category::Snapshots => "snapshots",
        }
    }

    /// Whether the category needs administrator rights to clean.
    pub fn system(self) -> bool {
        matches!(
            self,
            Category::PkgCache
                | Category::OrphanPkgs
                | Category::Journal
                | Category::Coredumps
                | Category::SystemTemp
                | Category::FlatpakUnusedSystem
                | Category::Snapshots
        )
    }

    /// Whether the category is enabled when the config doesn't say.
    pub fn default_on(self) -> bool {
        !matches!(self, Category::DevCache | Category::BuildOutput)
    }
}

impl FromStr for Category {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, String> {
        Category::ALL
            .into_iter()
            .find(|c| c.as_str() == s)
            .ok_or_else(|| format!("unknown category '{s}'"))
    }
}

/// How an item is removed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ItemAction {
    Trash,
    Delete,
    Command {
        program: String,
        args: Vec<String>,
    },
    /// Run as root in the privileged batch; `program` is a bare name.
    Privileged {
        program: String,
        args: Vec<String>,
    },
}

fn paths_only<S: Serializer>(v: &[(PathBuf, Fingerprint)], s: S) -> Result<S::Ok, S::Error> {
    let mut seq = s.serialize_seq(Some(v.len()))?;
    for (p, _) in v {
        seq.serialize_element(p)?;
    }
    seq.end()
}

/// One removable thing a scanner found.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BloatItem {
    pub category: Category,
    /// In-system; `None` only for `Command` and `Privileged` items.
    pub path: Option<PathBuf>,
    /// Display string (the description of a command item).
    pub label: String,
    pub size: Option<u64>,
    pub rebuildable: bool,
    pub preselected: bool,
    pub note: Option<String>,
    pub action: ItemAction,
    pub scope: Scope,
    #[serde(skip)]
    pub fingerprint: Option<Fingerprint>,
    /// Further paths removed with the item (a trashed entry's `.trashinfo`).
    #[serde(serialize_with = "paths_only")]
    pub extra_paths: Vec<(PathBuf, Fingerprint)>,
    pub app_id: Option<String>,
}

pub struct ScanContext<'a> {
    pub env: &'a Env,
    pub cfg: &'a Config,
    pub inv: &'a Inventory,
    pub runner: &'a dyn CommandRunner,
    pub now: SystemTime,
}

pub trait Scanner: Send + Sync {
    fn category(&self) -> Category;
    fn scan(&self, cx: &ScanContext) -> Result<Vec<BloatItem>, CoreError>;
    /// Why the category cannot be scanned here (a missing tool), checked
    /// before [`Scanner::scan`]. Unavailable is not an error.
    fn unavailable(&self, _cx: &ScanContext) -> Option<String> {
        None
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct CategoryReport {
    pub category: Category,
    pub items: Vec<BloatItem>,
    pub total: u64,
    pub unknown_sizes: usize,
    pub error: Option<String>,
    /// Listed but not scanned (a disabled category nobody asked for).
    /// Always false from [`scan_all`]; the CLI sets it.
    #[serde(default)]
    pub skipped: bool,
    /// The category cannot run here, for example `pacman not installed`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unavailable: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ScanReport {
    pub categories: Vec<CategoryReport>,
}

/// The implemented scanners, in [`Category::ALL`] order.
pub fn scanners() -> Vec<Box<dyn Scanner>> {
    vec![
        Box::new(cache::CacheScanner),
        Box::new(cache::ThumbnailsScanner),
        Box::new(trash::TrashScanner),
        Box::new(temp::TempScanner),
        Box::new(browser::BrowserCacheScanner),
        Box::new(dev::DevCacheScanner),
        Box::new(flatpak::FlatpakUnusedScanner),
        Box::new(appimage::AppimageOldScanner),
        Box::new(logs::LogsScanner),
        Box::new(aur::AurBuildScanner),
        Box::new(app::AppCacheScanner),
        Box::new(containers::ContainersScanner),
        Box::new(build::BuildOutputScanner),
        Box::new(system::PkgCacheScanner),
        Box::new(system::OrphanPkgsScanner),
        Box::new(system::JournalScanner),
        Box::new(system::CoredumpsScanner),
        Box::new(system::SystemTempScanner),
        Box::new(system::FlatpakUnusedSystemScanner),
        Box::new(system::SnapshotsScanner),
    ]
}

/// Scans the selected categories with the registered scanners.
pub fn scan_all(cx: &ScanContext, which: &[Category]) -> ScanReport {
    scan_with(cx, &scanners(), which)
}

/// Runs the scanners whose category is in `which` in parallel, measures the
/// items that lack a size, and sorts each category by size.
fn scan_with(cx: &ScanContext, scanners: &[Box<dyn Scanner>], which: &[Category]) -> ScanReport {
    let mut categories: Vec<CategoryReport> = scanners
        .par_iter()
        .filter(|s| which.contains(&s.category()))
        .map(|s| {
            let unavailable = s.unavailable(cx);
            let (items, error) = if unavailable.is_some() {
                (Vec::new(), None)
            } else {
                match s.scan(cx) {
                    Ok(items) => (items, None),
                    Err(e) => (Vec::new(), Some(e.to_string())),
                }
            };
            CategoryReport {
                category: s.category(),
                items,
                total: 0,
                unknown_sizes: 0,
                error,
                skipped: false,
                unavailable,
            }
        })
        .collect();

    // One parallel pass over every item that needs measuring.
    let mut slots: Vec<(usize, usize)> = Vec::new();
    let mut paths: Vec<PathBuf> = Vec::new();
    for (ci, rep) in categories.iter().enumerate() {
        for (ii, item) in rep.items.iter().enumerate() {
            if item.size.is_none()
                && matches!(item.action, ItemAction::Trash | ItemAction::Delete)
                && let Some(p) = &item.path
            {
                slots.push((ci, ii));
                paths.push(p.clone());
            }
        }
    }
    for ((ci, ii), usage) in slots.into_iter().zip(disk_usage_many(cx.env, &paths)) {
        categories[ci].items[ii].size = Some(usage.bytes);
    }

    for rep in &mut categories {
        rep.items.sort_by(|a, b| {
            let by_size = match (a.size, b.size) {
                (Some(x), Some(y)) => y.cmp(&x),
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => std::cmp::Ordering::Equal,
            };
            by_size.then_with(|| a.path.cmp(&b.path))
        });
        rep.total = rep.items.iter().filter_map(|i| i.size).sum();
        rep.unknown_sizes = rep.items.iter().filter(|i| i.size.is_none()).count();
    }
    ScanReport { categories }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::CmdOutput;
    use crate::testing::{FakeRunner, FixtureTree};
    use std::str::FromStr;

    #[test]
    fn category_round_trip_and_defaults() {
        let names = [
            "cache",
            "thumbnails",
            "trash",
            "temp",
            "browser-cache",
            "dev-cache",
            "flatpak-unused",
            "appimage-old",
            "logs",
            "aur-build",
            "app-cache",
            "containers",
            "build-output",
            "pkg-cache",
            "orphan-pkgs",
            "journal",
            "coredumps",
            "system-temp",
            "flatpak-unused-system",
            "snapshots",
        ];
        assert_eq!(Category::ALL.len(), 20);
        for (c, n) in Category::ALL.iter().zip(names) {
            assert_eq!(c.as_str(), n);
            assert_eq!(Category::from_str(n).unwrap(), *c);
            assert_eq!(serde_json::to_string(c).unwrap(), format!("\"{n}\""));
            assert_eq!(
                serde_json::from_str::<Category>(&format!("\"{n}\"")).unwrap(),
                *c
            );
            assert_eq!(
                c.default_on(),
                !matches!(c, Category::DevCache | Category::BuildOutput)
            );
            // The system categories come last, from `pkg-cache` on.
            let first_system = names.iter().position(|x| *x == "pkg-cache").unwrap();
            assert_eq!(
                c.system(),
                names.iter().position(|x| x == &n).unwrap() >= first_system
            );
        }
        assert!(Category::from_str("nope").is_err());
        assert!(Category::ALL.windows(2).all(|w| w[0] < w[1]));
    }

    struct Fixed(Category, Vec<BloatItem>);
    impl Scanner for Fixed {
        fn category(&self) -> Category {
            self.0
        }
        fn scan(&self, _: &ScanContext) -> Result<Vec<BloatItem>, CoreError> {
            Ok(self.1.clone())
        }
    }
    struct Failing(Category);
    impl Scanner for Failing {
        fn category(&self) -> Category {
            self.0
        }
        fn scan(&self, _: &ScanContext) -> Result<Vec<BloatItem>, CoreError> {
            Err(CoreError::Command {
                program: "x".into(),
                message: "boom".into(),
            })
        }
    }

    fn item(cat: Category, path: &str, size: Option<u64>) -> BloatItem {
        BloatItem {
            category: cat,
            path: Some(path.into()),
            label: path.into(),
            size,
            rebuildable: true,
            preselected: true,
            note: None,
            action: ItemAction::Trash,
            scope: Scope::Home,
            fingerprint: None,
            extra_paths: vec![(
                "/home/user/extra".into(),
                Fingerprint {
                    dev: 1,
                    ino: 2,
                    is_symlink: false,
                    ctime: 0,
                    ctime_nsec: 0,
                },
            )],
            app_id: None,
        }
    }

    #[test]
    fn scan_all_isolates_scanner_error() {
        let t = FixtureTree::new();
        t.write("/home/user/.cache/small", &[1u8; 5000]);
        t.write("/home/user/.cache/big", &[1u8; 50_000]);
        let env = t.env();
        let cfg = Config::default();
        let inv = Inventory {
            apps: vec![],
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
        let mut cmd = item(Category::Cache, "/x", None);
        cmd.path = None;
        cmd.action = ItemAction::Command {
            program: "flatpak".into(),
            args: vec![],
        };
        let scanners: Vec<Box<dyn Scanner>> = vec![
            Box::new(Failing(Category::Trash)),
            Box::new(Fixed(
                Category::Cache,
                vec![
                    item(Category::Cache, "/home/user/.cache/small", None),
                    item(Category::Cache, "/home/user/.cache/missing", None),
                    item(Category::Cache, "/home/user/.cache/big", None),
                    item(Category::Cache, "/home/user/.cache/known", Some(7)),
                    cmd,
                ],
            )),
        ];
        let rep = scan_with(&cx, &scanners, &[Category::Trash, Category::Cache]);
        assert_eq!(rep.categories.len(), 2);
        let trash = &rep.categories[0];
        assert_eq!(trash.category, Category::Trash);
        assert!(trash.error.as_deref().unwrap().contains("boom"));
        assert!(trash.items.is_empty());
        let cache = &rep.categories[1];
        assert!(cache.error.is_none());
        let paths: Vec<_> = cache.items.iter().map(|i| i.path.clone()).collect();
        assert_eq!(paths[0], Some("/home/user/.cache/big".into()));
        assert_eq!(paths[1], Some("/home/user/.cache/small".into()));
        assert_eq!(paths[2], Some("/home/user/.cache/known".into()));
        assert_eq!(cache.items[2].size, Some(7));
        // Missing path measures 0 (with an error), command item stays None.
        assert_eq!(
            cache.items[3].path,
            Some("/home/user/.cache/missing".into())
        );
        assert_eq!(cache.items[3].size, Some(0));
        assert_eq!(cache.items[4].size, None);
        assert_eq!(cache.unknown_sizes, 1);
        assert_eq!(
            cache.total,
            cache.items.iter().filter_map(|i| i.size).sum::<u64>()
        );
        // `which` filters; unselected scanners don't run.
        let only = scan_with(&cx, &scanners, &[Category::Cache]);
        assert_eq!(only.categories.len(), 1);
        let _ = CmdOutput {
            status: 0,
            stdout: String::new(),
            stderr: String::new(),
            timed_out: false,
        };
    }

    #[test]
    fn extra_paths_serialize_as_paths_only() {
        let v = serde_json::to_value(item(Category::Trash, "/home/user/a", Some(1))).unwrap();
        assert_eq!(v["extra_paths"], serde_json::json!(["/home/user/extra"]));
        assert!(v.get("fingerprint").is_none());
        assert_eq!(v["category"], "trash");
    }
}
