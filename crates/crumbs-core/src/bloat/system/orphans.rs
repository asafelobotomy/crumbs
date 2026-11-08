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
        resolve_trusted(cx.env, "pacman")
            .is_none()
            .then(|| not_installed("pacman"))
    }

    fn scan(&self, cx: &ScanContext) -> Result<Vec<BloatItem>, CoreError> {
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
