#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use super::*;
use crate::model::{App, SourceKind};
use crate::safety::{is_allowed_target, verify};
use crate::testing::{FakeRunner, FixtureTree};

fn inv(apps: Vec<App>) -> Inventory {
    Inventory {
        apps,
        statuses: vec![],
        errors: vec![],
        owned: Default::default(),
    }
}

fn run(t: &FixtureTree, cfg: &Config, inv: &Inventory, cat: Category) -> Vec<BloatItem> {
    let env = t.env();
    let runner = FakeRunner::new();
    let cx = ScanContext {
        env: &env,
        cfg,
        inv,
        runner: &runner,
        now: test_now(),
    };
    scanners()
        .into_iter()
        .find(|s| s.category() == cat)
        .unwrap()
        .scan(&cx)
        .unwrap()
}

/// The scans' clock in these tests: 100 days ahead, so an entry made now
/// already has a 100-day-old ctime (which cannot be set) and its mtime,
/// set relative to this clock, decides its age.
const CLOCK_SHIFT: Duration = Duration::from_secs(100 * 86_400);

fn test_now() -> SystemTime {
    SystemTime::now() + CLOCK_SHIFT
}

fn paths(items: &[BloatItem]) -> Vec<PathBuf> {
    items.iter().map(|i| i.path.clone().unwrap()).collect()
}

fn check_common(t: &FixtureTree, cfg: &Config, items: &[BloatItem]) {
    let env = t.env();
    for i in items {
        let p = i.path.as_ref().unwrap();
        assert_eq!(i.scope, Scope::Home);
        assert_eq!(i.action, ItemAction::Trash);
        assert_eq!(i.size, None);
        assert!(is_allowed_target(&env, cfg, Scope::Home, p));
        verify(&env, cfg, Scope::Home, p, i.fingerprint.as_ref().unwrap()).unwrap();
    }
}

fn age(p: &Path, days: u64) {
    let f = fs::File::options().write(true).open(p).unwrap();
    f.set_modified(test_now() - Duration::from_secs(days * 86_400 + 3600))
        .unwrap();
}

// ---- browser-cache, temp, trash ------------------------------------------

use crate::command::CmdOutput;

fn scan_one(
    t: &FixtureTree,
    cfg: &Config,
    runner: &FakeRunner,
    cat: Category,
) -> Result<Vec<BloatItem>, CoreError> {
    let env = t.env();
    let i = inv(vec![]);
    let cx = ScanContext {
        env: &env,
        cfg,
        inv: &i,
        runner,
        now: test_now(),
    };
    scanners()
        .into_iter()
        .find(|s| s.category() == cat)
        .unwrap()
        .scan(&cx)
}

/// [`scan_one`] at the real time, unshifted.
fn scan_real_now(
    t: &FixtureTree,
    cfg: &Config,
    runner: &FakeRunner,
    cat: Category,
) -> Result<Vec<BloatItem>, CoreError> {
    let env = t.env();
    let i = inv(vec![]);
    let cx = ScanContext {
        env: &env,
        cfg,
        inv: &i,
        runner,
        now: SystemTime::now(),
    };
    scanners()
        .into_iter()
        .find(|s| s.category() == cat)
        .unwrap()
        .scan(&cx)
}

fn labels(items: &[BloatItem]) -> Vec<&str> {
    items.iter().map(|i| i.label.as_str()).collect()
}

/// Sets atime and mtime of `p` (never following a symlink) to `days` (and
/// an hour) before [`test_now`].
fn age_nofollow(p: &Path, days: u64) {
    use rustix::fs::{AtFlags, CWD, Timespec, Timestamps, utimensat};
    let secs = test_now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_secs()
        - days * 86_400
        - 3600;
    let ts = Timespec {
        tv_sec: secs as i64,
        tv_nsec: 0,
    };
    utimensat(
        CWD,
        p,
        &Timestamps {
            last_access: ts,
            last_modification: ts,
        },
        AtFlags::SYMLINK_NOFOLLOW,
    )
    .unwrap();
}

fn check_browser(t: &FixtureTree, items: &[BloatItem]) {
    let cfg = Config::default();
    check_common(t, &cfg, items);
    for i in items {
        assert_eq!(i.category, Category::BrowserCache);
        assert!(i.rebuildable);
        assert!(i.extra_paths.is_empty());
    }
}

fn check_temp(t: &FixtureTree, items: &[BloatItem]) {
    let env = t.env();
    let cfg = Config::default();
    for i in items {
        let p = i.path.as_ref().unwrap();
        assert_eq!(i.category, Category::Temp);
        assert_eq!(i.scope, Scope::UserTemp);
        assert_eq!(i.action, ItemAction::Delete);
        assert_eq!(
            i.note.as_deref(),
            Some("deleted permanently (temporary file)")
        );
        assert!(!i.rebuildable && i.preselected);
        assert!(is_allowed_target(&env, &cfg, Scope::UserTemp, p));
        verify(
            &env,
            &cfg,
            Scope::UserTemp,
            p,
            i.fingerprint.as_ref().unwrap(),
        )
        .unwrap();
    }
}

fn check_trash(t: &FixtureTree, items: &[BloatItem]) {
    let env = t.env();
    let cfg = Config::default();
    for i in items {
        assert_eq!(i.category, Category::Trash);
        assert_eq!(i.scope, Scope::HomeTrash);
        assert_eq!(i.action, ItemAction::Delete);
        assert!(
            !i.rebuildable && !i.preselected,
            "emptying the trash is opt-in"
        );
        let p = i.path.as_ref().unwrap();
        verify(
            &env,
            &cfg,
            Scope::HomeTrash,
            p,
            i.fingerprint.as_ref().unwrap(),
        )
        .unwrap();
        for (x, fp) in &i.extra_paths {
            verify(&env, &cfg, Scope::HomeTrash, x, fp).unwrap();
        }
    }
}

mod app_thumbnails;
mod cache_aur;
mod flatpak_temp;
mod sizes;
mod temp_trash;
