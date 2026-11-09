//! Human and JSON rendering helpers.

use std::fmt;
use std::io::{self, Write as _};

use anstyle::Style;
use crumbs_core::SourceError;
use crumbs_core::inventory::{Inventory, SourceStatus};
use crumbs_core::leftovers::{Confidence, FindingKind};
use serde::Serialize;

const SCHEMA_VERSION: u32 = 1;

/// Some source failed while at least one other source worked.
pub fn partial_failure(inv: &Inventory) -> bool {
    partial_failure_parts(&inv.statuses, &inv.errors)
}

pub fn partial_failure_parts(statuses: &[SourceStatus], errors: &[SourceError]) -> bool {
    !errors.is_empty()
        && statuses.iter().any(|s| {
            s.available
                && s.skipped_reason.is_none()
                && !errors.iter().any(|e| e.source == s.source)
        })
}

/// The exit code a listing's source failures give: 1 when every available
/// source failed, 3 when only some did, else 0.
pub fn source_exit_code(statuses: &[SourceStatus], errors: &[SourceError]) -> u8 {
    if errors.is_empty() {
        0
    } else if partial_failure_parts(statuses, errors) {
        3
    } else {
        1
    }
}

/// Formats a byte count in IEC units with one decimal place.
pub fn fmt_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["KiB", "MiB", "GiB", "TiB", "PiB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64 / 1024.0;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    format!("{value:.1} {}", UNITS[unit])
}

#[derive(Serialize)]
struct Envelope<'a, T: Serialize> {
    schema_version: u32,
    command: &'a str,
    data: &'a T,
    errors: &'a [ErrorEntry],
}

/// One envelope `errors` entry. `source` is a source name, or `config` /
/// `rules` for notes built in the CLI.
#[derive(Serialize, Debug, Clone)]
pub struct ErrorEntry {
    pub source: String,
    pub message: String,
}

impl ErrorEntry {
    pub fn new(source: &str, message: String) -> Self {
        ErrorEntry {
            source: source.into(),
            message,
        }
    }
}

impl From<&SourceError> for ErrorEntry {
    fn from(e: &SourceError) -> Self {
        ErrorEntry::new(e.source.as_str(), e.message.clone())
    }
}

/// Serde's own name for a value that serializes to a plain string.
fn serde_name<T: Serialize>(v: T) -> String {
    serde_json::to_value(v)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_else(|| "-".into())
}

pub fn confidence(c: Confidence) -> String {
    serde_name(c)
}

pub fn finding_kind(k: FindingKind) -> String {
    serde_name(k)
}

/// Writes the JSON envelope for one invocation to stdout.
pub fn print_json_entries<T: Serialize>(
    command: &str,
    data: &T,
    errors: &[ErrorEntry],
) -> anyhow::Result<()> {
    let envelope = Envelope {
        schema_version: SCHEMA_VERSION,
        command,
        data,
        errors,
    };
    let mut out = anstream::stdout().lock();
    serde_json::to_writer_pretty(&mut out, &envelope)?;
    writeln!(out)?;
    Ok(())
}

/// Renders a table with padded columns and a bold header row. The last
/// column is not padded, so lines carry no trailing spaces.
pub fn print_table(header: &[&str], rows: &[Vec<String>]) -> io::Result<()> {
    print_table_with_details(header, rows, &[])
}

/// Like [`print_table`], with `details[i]` printed under row `i`, one per
/// line, as `    - <detail>`. Missing entries mean no details.
pub fn print_table_with_details(
    header: &[&str],
    rows: &[Vec<String>],
    details: &[Vec<String>],
) -> io::Result<()> {
    let mut text = String::new();
    write_table_with_details(&mut text, header, rows, details, true).map_err(io::Error::other)?;
    anstream::stdout().lock().write_all(text.as_bytes())
}

/// [`print_table_with_details`] into `w`; the header is bold only with
/// `color`.
pub fn write_table_with_details(
    w: &mut dyn fmt::Write,
    header: &[&str],
    rows: &[Vec<String>],
    details: &[Vec<String>],
    color: bool,
) -> fmt::Result {
    let mut widths: Vec<usize> = header.iter().map(|h| h.chars().count()).collect();
    for row in rows {
        for (w, cell) in widths.iter_mut().zip(row) {
            *w = (*w).max(cell.chars().count());
        }
    }
    let bold = if color {
        Style::new().bold()
    } else {
        Style::new()
    };
    let last = header.len().saturating_sub(1);
    let line = |cells: &mut dyn Iterator<Item = &str>| -> String {
        let mut s = String::new();
        for (i, cell) in cells.enumerate() {
            s.push_str(cell);
            if i < last {
                let pad = widths[i].saturating_sub(cell.chars().count());
                s.extend(std::iter::repeat_n(' ', pad + 2));
            }
        }
        s.truncate(s.trim_end().len());
        s
    };
    let head = line(&mut header.iter().copied());
    writeln!(w, "{bold}{head}{bold:#}")?;
    for (i, row) in rows.iter().enumerate() {
        writeln!(w, "{}", line(&mut row.iter().map(String::as_str)))?;
        for d in details.get(i).into_iter().flatten() {
            writeln!(w, "    - {d}")?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crumbs_core::SourceKind;

    #[test]
    fn fmt_size_iec() {
        assert_eq!(fmt_size(0), "0 B");
        assert_eq!(fmt_size(1023), "1023 B");
        assert_eq!(fmt_size(1024), "1.0 KiB");
        assert_eq!(fmt_size(1_572_864), "1.5 MiB");
    }

    fn status(source: SourceKind, available: bool) -> SourceStatus {
        SourceStatus {
            source,
            available,
            app_count: 0,
            skipped_reason: (!available).then(|| "not installed".to_string()),
        }
    }

    fn failed(source: SourceKind) -> SourceError {
        SourceError {
            source,
            message: "boom".into(),
        }
    }

    /// Unreachable through the CLI today (the AppImage source cannot
    /// fail), so pinned here.
    #[test]
    fn all_sources_failed_exit_1() {
        let statuses = [
            status(SourceKind::Pacman, true),
            status(SourceKind::Flatpak, true),
            status(SourceKind::Unmanaged, false),
        ];
        let both = [failed(SourceKind::Pacman), failed(SourceKind::Flatpak)];
        assert_eq!(source_exit_code(&statuses, &both), 1);
        assert_eq!(source_exit_code(&statuses, &both[..1]), 3);
        assert_eq!(source_exit_code(&statuses, &[]), 0);
        // Nothing available and nothing failed is no failure.
        assert_eq!(
            source_exit_code(&[status(SourceKind::Pacman, false)], &[]),
            0
        );
    }
}
