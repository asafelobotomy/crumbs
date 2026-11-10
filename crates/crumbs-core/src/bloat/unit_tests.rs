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
