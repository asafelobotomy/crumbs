#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use super::*;
use crate::config::Config;
use crate::inventory::Inventory;
use crate::model::{App, Installation, SourceKind};
use crate::rules::{Rule, RuleOrigin, load_rules};
use crate::safety::fingerprint;
use crate::testing::FixtureTree;

const HOME: &str = "/home/user";

fn inv(apps: Vec<App>) -> Inventory {
    Inventory {
        apps,
        statuses: Vec::new(),
        errors: Vec::new(),
        owned: Default::default(),
    }
}

fn appimage(file: &str) -> App {
    let (base, version) = crate::sources::appimage::split_appimage_name(file);
    let mut a = App::new(SourceKind::AppImage, file);
    a.id = format!("appimage:~/Applications/{file}");
    a.display_name = base;
    a.version = version;
    a.installation = Installation::User;
    a.location = Some(PathBuf::from(format!("{HOME}/Applications/{file}")));
    a
}

fn pacman(name: &str, exec: &[&str]) -> App {
    let mut a = App::new(SourceKind::Pacman, name);
    a.exec_names = exec.iter().map(|s| s.to_string()).collect();
    a
}

fn flatpak(id: &str) -> App {
    let mut a = App::new(SourceKind::Flatpak, id);
    a.id = format!("flatpak:user/{id}");
    a.rdns_id = Some(id.to_string());
    a.installation = Installation::User;
    a.desktop_ids = vec![format!("{id}.desktop")];
    a
}

fn user_rule(packages: &[&str], paths: &[&str]) -> Rule {
    Rule {
        packages: packages.iter().map(|s| s.to_string()).collect(),
        flatpaks: Vec::new(),
        appimages: Vec::new(),
        desktops: Vec::new(),
        paths: paths.iter().map(|s| s.to_string()).collect(),
        origin: RuleOrigin::User(PathBuf::from("/home/user/.config/crumbs/rules.d/t.toml")),
    }
}

struct Run {
    idx: LeftoverIndex,
    inv: Inventory,
}

impl Run {
    fn new(t: &FixtureTree, apps: Vec<App>) -> Run {
        Run::with(t, apps, &[], &Config::default())
    }

    fn with(t: &FixtureTree, apps: Vec<App>, rules: &[Rule], cfg: &Config) -> Run {
        let inv = inv(apps);
        let idx = LeftoverIndex::build(&t.env(), cfg, rules, &inv);
        Run { idx, inv }
    }

    fn find(&self, i: usize, show_low: bool) -> Vec<Finding> {
        find_leftovers(&self.idx, &self.inv.apps[i], show_low)
    }
}

fn at<'a>(fs: &'a [Finding], rel: &str) -> Option<&'a Finding> {
    let p = Path::new(HOME).join(rel);
    fs.iter().find(|f| f.path == p)
}

fn must<'a>(fs: &'a [Finding], rel: &str) -> &'a Finding {
    at(fs, rel).unwrap_or_else(|| panic!("no finding for ~/{rel}: {fs:#?}"))
}

const DAY: Duration = Duration::from_secs(86_400);

fn orphans_at(t: &FixtureTree, apps: Vec<App>, show_low: bool, now: SystemTime) -> Vec<Finding> {
    let (rules, _) = load_rules(&t.env());
    let r = Run::with(t, apps, &rules, &Config::default());
    super::orphans::find_orphans_at(&r.idx, show_low, now)
}

/// Orphans with the clock 60 days ahead, so fixture files count as old.
fn orphans(t: &FixtureTree, apps: Vec<App>, show_low: bool) -> Vec<Finding> {
    orphans_at(t, apps, show_low, SystemTime::now() + DAY * 60)
}

fn set_age(t: &FixtureTree, rel: &str, days: u64) {
    let f = std::fs::File::open(t.mkdir(rel)).unwrap();
    f.set_modified(SystemTime::now() - DAY * days as u32)
        .unwrap();
}

mod aur_data;
mod exact_normalized;
mod icon_mimeapps;
mod snap_uninstalled;
mod sys;
