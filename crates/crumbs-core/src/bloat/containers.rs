//! `containers`: what podman and Docker keep that no container uses.
//! Untagged ("dangling") images are one command item per engine, removed by
//! the engine itself with `<engine> image prune -f`, so the rule that lists
//! them is the rule that removes them. Unused volumes are one item each
//! (`<engine> volume rm <name>`), never preselected: a volume can hold data.
//! An engine that is missing, or that this user cannot reach (Docker
//! without access to its daemon), lists nothing.

use std::fs;
use std::path::Path;

use serde_json::Value;

use super::{BloatItem, Category, ItemAction, ScanContext, Scanner};
use crate::command::resolve_trusted;
use crate::error::CoreError;
use crate::safety::Scope;
use crate::size::disk_usage;

const ENGINES: [&str; 2] = ["podman", "docker"];

const VOLUME_NOTE: &str = "used by no container, but a volume can hold data you want";

pub struct ContainersScanner;

impl Scanner for ContainersScanner {
    fn category(&self) -> Category {
        Category::Containers
    }

    fn scan(&self, cx: &ScanContext) -> Result<Vec<BloatItem>, CoreError> {
        let mut out = Vec::new();
        for engine in ENGINES {
            let Some(program) = resolve_trusted(cx.env, engine) else {
                continue;
            };
            let query = |args: &[&str]| {
                cx.runner
                    .run(&program, args)
                    .ok()
                    .filter(|o| o.status == 0 && !o.timed_out)
                    .map(|o| records(&o.stdout))
            };
            if let Some(images) =
                query(&["images", "--filter", "dangling=true", "--format", "json"])
                && !images.is_empty()
            {
                let size = images
                    .iter()
                    .map(|i| size_of(&i["Size"]))
                    .sum::<Option<u64>>();
                let n = images.len();
                let what = if n == 1 { "image" } else { "images" };
                out.push(command_item(
                    format!("{n} untagged {engine} {what}"),
                    size,
                    true,
                    Some("layers shared with other images stay; what is freed is measured".into()),
                    engine,
                    &["image", "prune", "-f"],
                ));
            }
            for vol in query(&[
                "volume",
                "ls",
                "--filter",
                "dangling=true",
                "--format",
                "json",
            ])
            .unwrap_or_default()
            {
                let Some(name) = vol["Name"].as_str().filter(|n| valid_volume_name(n)) else {
                    continue;
                };
                let size = vol["Mountpoint"]
                    .as_str()
                    .map(Path::new)
                    .filter(|m| m.is_absolute() && fs::symlink_metadata(cx.env.sys_path(m)).is_ok())
                    .map(|m| disk_usage(cx.env, m).bytes);
                let mut item = command_item(
                    format!("{engine} volume {name}"),
                    size,
                    false,
                    Some(VOLUME_NOTE.into()),
                    engine,
                    &["volume", "rm", name],
                );
                item.preselected = false;
                out.push(item);
            }
        }
        Ok(out)
    }
}

fn command_item(
    label: String,
    size: Option<u64>,
    rebuildable: bool,
    note: Option<String>,
    program: &str,
    args: &[&str],
) -> BloatItem {
    BloatItem {
        category: Category::Containers,
        path: None,
        label,
        size,
        rebuildable,
        preselected: true,
        note,
        action: ItemAction::Command {
            program: program.into(),
            args: args.iter().map(|a| (*a).to_string()).collect(),
        },
        scope: Scope::Home,
        fingerprint: None,
        extra_paths: Vec::new(),
        app_id: None,
    }
}

/// The records of `--format json`: podman prints one JSON array, Docker
/// one object per line.
fn records(stdout: &str) -> Vec<Value> {
    let text = stdout.trim();
    if text.starts_with('[') {
        return serde_json::from_str::<Vec<Value>>(text).unwrap_or_default();
    }
    text.lines()
        .filter_map(|l| serde_json::from_str::<Value>(l.trim()).ok())
        .filter(Value::is_object)
        .collect()
}

/// An image size: bytes (podman), or Docker's decimal text (`1.5GB`).
fn size_of(v: &Value) -> Option<u64> {
    if let Some(n) = v.as_u64() {
        return Some(n);
    }
    let s = v.as_str()?.trim();
    let split = s.find(|c: char| !(c.is_ascii_digit() || c == '.'))?;
    let (num, unit) = s.split_at(split);
    let num: f64 = num.parse().ok()?;
    let scale = match unit.trim() {
        "B" => 1.0,
        "kB" | "KB" => 1e3,
        "MB" => 1e6,
        "GB" => 1e9,
        "TB" => 1e12,
        _ => return None,
    };
    Some((num * scale).round() as u64)
}

/// podman's and Docker's rule for volume names, `[a-zA-Z0-9][a-zA-Z0-9_.-]*`,
/// so a name can never be read as an option.
fn valid_volume_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphanumeric())
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
}

#[cfg(test)]
#[path = "containers_tests.rs"]
mod tests;
