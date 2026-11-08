//! System categories: things only root can clean. Scanners run as the user
//! and change nothing; the `Privileged` items they return are executed later
//! by the privileged batch (the program is a bare name, resolved then).

mod coredumps;
mod flatpak;
mod journal;
mod orphans;
mod pkg_cache;
#[cfg(test)]
mod tests;
mod tmpfiles;

pub use coredumps::CoredumpsScanner;
pub use flatpak::FlatpakUnusedSystemScanner;
pub use journal::JournalScanner;
pub use orphans::OrphanPkgsScanner;
pub use pkg_cache::PkgCacheScanner;
pub use tmpfiles::SystemTempScanner;

use super::{BloatItem, Category, ItemAction};
use crate::command::CmdOutput;
use crate::error::CoreError;
use crate::safety::Scope;

/// The reason string of a category whose tool is missing.
pub(super) fn not_installed(tool: &str) -> String {
    format!("{tool} not installed")
}

/// A `Privileged` item: no path, no fingerprint, a bare program name.
pub(super) fn privileged_item(
    category: Category,
    label: String,
    size: Option<u64>,
    note: Option<String>,
    program: &str,
    args: Vec<String>,
) -> BloatItem {
    BloatItem {
        category,
        path: None,
        label,
        size,
        rebuildable: false,
        preselected: true,
        note,
        action: ItemAction::Privileged {
            program: program.into(),
            args,
        },
        scope: Scope::System,
        fingerprint: None,
        extra_paths: Vec::new(),
        app_id: None,
    }
}

/// The error for a query that exited non-zero.
pub(super) fn command_failed(program: &str, out: &CmdOutput) -> CoreError {
    let detail = out.stderr.trim();
    CoreError::Command {
        program: program.into(),
        message: if out.timed_out {
            "timed out".into()
        } else if detail.is_empty() {
            format!("exited with status {}", out.status)
        } else {
            detail.to_string()
        },
    }
}

/// `value` times the power of 1024 named by `unit` (`B`, `K`/`KiB`, `M`/
/// `MiB`, `G`/`GiB`, `T`/`TiB`), rounded. `None` for anything else.
pub(super) fn scale_1024(value: f64, unit: &str) -> Option<u64> {
    let power = match unit {
        "" | "B" => 0,
        "K" | "KiB" => 1,
        "M" | "MiB" => 2,
        "G" | "GiB" => 3,
        "T" | "TiB" => 4,
        _ => return None,
    };
    let bytes = value * 1024f64.powi(power);
    (bytes.is_finite() && bytes >= 0.0 && bytes < u64::MAX as f64).then(|| bytes.round() as u64)
}

/// Parses `86.17 MiB` or `719.1M`: a decimal number, optional spaces, a unit.
pub(super) fn parse_size(text: &str) -> Option<u64> {
    let text = text.trim();
    let split = text
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .unwrap_or(text.len());
    let (num, unit) = text.split_at(split);
    scale_1024(num.parse().ok()?, unit.trim())
}
