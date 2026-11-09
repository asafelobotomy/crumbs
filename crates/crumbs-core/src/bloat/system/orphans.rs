//! `orphan-pkgs`: packages installed as dependencies that nothing needs any
//! more. Opt-in (they are often optional dependencies the user wants). The
//! removal is simulated first and the item removes exactly that closure.

use std::collections::HashMap;
use std::fs;

use super::super::{BloatItem, Category, ScanContext, Scanner};
use super::{command_failed, not_installed, privileged_item};
use crate::command::resolve_trusted;
use crate::error::CoreError;
use crate::removal::{refusal_text, valid_name};
use crate::sources::pacman::{first, parse_sections};

const DB_DIR: &str = "/var/lib/pacman/local";
const NOTE_NAMES: usize = 5;

pub struct OrphanPkgsScanner;

impl Scanner for OrphanPkgsScanner {
    fn category(&self) -> Category {
        Category::OrphanPkgs
    }

    fn unavailable(&self, cx: &ScanContext) -> Option<String> {
        super::package_manager(cx.env)
            .is_none()
            .then(|| not_installed(super::NO_PACKAGE_MANAGER))
    }

    fn scan(&self, cx: &ScanContext) -> Result<Vec<BloatItem>, CoreError> {
        match super::package_manager(cx.env) {
            Some(super::PackageManager::AptGet) => return apt(cx),
            Some(super::PackageManager::Dnf) => return dnf(cx),
            _ => {}
        }
        let Some(pacman) = resolve_trusted(cx.env, "pacman") else {
            return Ok(Vec::new());
        };
        let out = cx.runner.run(&pacman, &["-Qdtq"])?;
        let orphans: Vec<String> = out
            .stdout
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .map(String::from)
            .collect();
        // pacman exits 1 with no output when there is nothing to list.
        if out.status != 0 && !(orphans.is_empty() && out.stderr.trim().is_empty()) {
            return Err(command_failed("pacman", &out));
        }
        if orphans.is_empty() {
            return Ok(Vec::new());
        }
        if let Some(bad) = orphans.iter().find(|n| !valid_name(n)) {
            return Err(CoreError::Parse {
                what: "pacman -Qdtq".into(),
                message: format!("refusing {bad:?}: not a plain package name"),
            });
        }

        let mut args = vec!["-Rs", "--print", "--print-format", "%n %v"];
        args.extend(orphans.iter().map(String::as_str));
        let sim = cx.runner.run(&pacman, &args)?;
        if sim.status != 0 {
            return Err(CoreError::Command {
                program: "pacman".into(),
                message: refusal_text(&sim),
            });
        }
        let mut closure: Vec<String> = Vec::new();
        for line in sim.stdout.lines().filter(|l| !l.trim().is_empty()) {
            let mut parts = line.split_whitespace();
            let (Some(name), Some(_version), None) = (parts.next(), parts.next(), parts.next())
            else {
                return Err(CoreError::Parse {
                    what: "pacman simulation".into(),
                    message: format!("unexpected line {line:?}"),
                });
            };
            if !valid_name(name) {
                return Err(CoreError::Parse {
                    what: "pacman simulation".into(),
                    message: format!("refusing {name:?}: not a plain package name"),
                });
            }
            if !closure.iter().any(|n| n == name) {
                closure.push(name.to_string());
            }
        }
        if closure.is_empty() {
            return Ok(Vec::new());
        }

        let sizes = installed_sizes(cx);
        let size: u64 = closure.iter().filter_map(|n| sizes.get(n)).sum();
        let mut note = closure
            .iter()
            .take(NOTE_NAMES)
            .map(String::as_str)
            .collect::<Vec<_>>()
            .join(", ");
        if closure.len() > NOTE_NAMES {
            note.push_str(", …");
        }
        // No -s: exactly the simulated list, so pacman never removes more.
        let mut run = vec!["-Rn".to_string(), "--noconfirm".to_string()];
        run.extend(closure.iter().cloned());
        let mut item = privileged_item(
            Category::OrphanPkgs,
            format!("{} orphaned packages", closure.len()),
            Some(size),
            Some(note),
            "pacman",
            run,
        );
        // Opt-in: listed, but only run when the category is named.
        item.preselected = false;
        Ok(vec![item])
    }
}

/// `%SIZE%` (installed bytes) by package name, from the local database.
/// Unreadable entries are skipped: the size is an estimate.
fn installed_sizes(cx: &ScanContext) -> HashMap<String, u64> {
    let mut sizes = HashMap::new();
    let Ok(rd) = fs::read_dir(cx.env.sys(DB_DIR)) else {
        return sizes;
    };
    for entry in rd.flatten() {
        let Ok(bytes) = fs::read(entry.path().join("desc")) else {
            continue;
        };
        let sections = parse_sections(&String::from_utf8_lossy(&bytes));
        if let (Some(name), Some(size)) = (first(&sections, "NAME"), first(&sections, "SIZE"))
            && let Ok(size) = size.parse::<u64>()
        {
            sizes.insert(name, size);
        }
    }
    sizes
}

fn removal_error(program: &str, e: crate::removal::RemovalError) -> CoreError {
    CoreError::Command {
        program: program.into(),
        message: e.to_string(),
    }
}

fn orphan_item(
    names: Vec<String>,
    size: Option<u64>,
    action: crate::plan::Action,
) -> Vec<BloatItem> {
    let crate::plan::Action::PackageRemove { program, args, .. } = action else {
        return Vec::new();
    };
    let mut note = names
        .iter()
        .take(NOTE_NAMES)
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join(", ");
    if names.len() > NOTE_NAMES {
        note.push_str(", …");
    }
    let mut item = privileged_item(
        Category::OrphanPkgs,
        format!("{} orphaned packages", names.len()),
        size,
        Some(note),
        &program,
        args,
    );
    item.preselected = false;
    vec![item]
}

/// Debian/Ubuntu: what `apt-get autoremove --purge` would remove, purged as
/// that exact list (never `autoremove` as root).
fn apt(cx: &ScanContext) -> Result<Vec<BloatItem>, CoreError> {
    use crate::removal::apt::{parse_simulation, purge_action};
    let Some(apt) = resolve_trusted(cx.env, "apt-get") else {
        return Ok(Vec::new());
    };
    let out = cx.runner.run(&apt, &["-s", "autoremove", "--purge"])?;
    let removed = parse_simulation(&out).map_err(|e| removal_error("apt-get", e))?;
    if removed.is_empty() {
        return Ok(Vec::new());
    }
    let sizes: HashMap<String, u64> = fs::read(cx.env.sys(crate::sources::dpkg::STATUS))
        .map(|b| {
            crate::sources::dpkg::parse_status(&String::from_utf8_lossy(&b))
                .into_iter()
                .filter_map(|s| Some((s.name, s.installed_kib? * 1024)))
                .collect()
        })
        .unwrap_or_default();
    let names: Vec<String> = removed.iter().map(|(n, _)| n.clone()).collect();
    let versions: Vec<String> = removed.iter().map(|(_, v)| v.clone()).collect();
    let size: u64 = names
        .iter()
        .filter_map(|n| sizes.get(n.split(':').next().unwrap_or(n)))
        .sum();
    let action = purge_action(names.clone(), versions);
    Ok(orphan_item(names, Some(size), action))
}

/// Fedora: what `dnf autoremove` would remove, removed as that exact list
/// with dependency clean-up off. The size is unknown (dnf's table rounds).
fn dnf(cx: &ScanContext) -> Result<Vec<BloatItem>, CoreError> {
    use crate::removal::dnf::{parse_transaction, remove_action};
    let Some(dnf) = resolve_trusted(cx.env, "dnf") else {
        return Ok(Vec::new());
    };
    let out = cx.runner.run(&dnf, &["autoremove", "--assumeno"])?;
    if out.stdout.lines().any(|l| l.trim() == "Nothing to do.") && !out.stdout.contains("Removing")
    {
        return Ok(Vec::new());
    }
    let removed = parse_transaction(&out).map_err(|e| removal_error("dnf", e))?;
    let names: Vec<String> = removed.iter().map(|(n, _)| n.clone()).collect();
    let versions: Vec<String> = removed.iter().map(|(_, v)| v.clone()).collect();
    let action = remove_action(names.clone(), versions);
    Ok(orphan_item(names, None, action))
}
