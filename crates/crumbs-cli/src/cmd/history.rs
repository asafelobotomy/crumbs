//! `history`: past batches, newest first.

use std::collections::HashSet;
use std::io::Write;
use std::process::ExitCode;

use crumbs_core::Env;
use crumbs_core::history::{
    HistoryAction, HistoryEntry, KIND_ORPHANS, KIND_UNINSTALL, read_history,
};
use serde::Serialize;
use serde_json::json;

use crate::output::{ErrorEntry, fmt_size, print_json_entries};

use super::render::sanitise;

#[derive(Serialize)]
struct Batch<'a> {
    batch: &'a str,
    time: &'a str,
    /// The app id, `orphans`, `clean`, or `restore` for a batch of restores.
    label: String,
    items: usize,
    /// Bytes of the entries that succeeded.
    size: u64,
    /// Entries a later successful restore put back.
    restored: usize,
    entries: Vec<&'a HistoryEntry>,
}

/// `2026-11-08T10:00:00.123Z` → `2026-11-08 10:00:00`; anything else as is.
fn short_time(t: &str) -> &str {
    if t.len() >= 19 && t.as_bytes().get(10) == Some(&b'T') {
        t.get(..19).unwrap_or(t)
    } else {
        t
    }
}

/// `restore` for a batch of restores; otherwise by the recorded kind: the
/// app id for an uninstall, else the kind itself. Entries written before
/// `kind` existed fall back to the app id or `orphans`.
fn label(es: &[&HistoryEntry]) -> String {
    if es.iter().all(|e| e.action == HistoryAction::Restore) {
        return "restore".to_string();
    }
    let app = || es.iter().find_map(|e| e.app_id.clone());
    match es.iter().find_map(|e| e.kind.as_deref()) {
        Some(KIND_UNINSTALL) | None => app().unwrap_or_else(|| KIND_ORPHANS.into()),
        Some(kind) => kind.to_string(),
    }
}

pub fn run(env: &Env, limit: usize, json: bool) -> anyhow::Result<ExitCode> {
    let (entries, skipped) = read_history(env)?;
    let restored: HashSet<&str> = entries
        .iter()
        .filter(|e| e.action == HistoryAction::Restore && e.ok)
        .filter_map(|e| e.restores.as_deref())
        .collect();
    // Batches in file order (the file is append-only), then newest first.
    let mut order: Vec<&str> = Vec::new();
    for e in &entries {
        if !order.contains(&e.batch.as_str()) {
            order.push(&e.batch);
        }
    }
    let batches: Vec<Batch> = order
        .iter()
        .rev()
        .take(limit)
        .map(|b| {
            let es: Vec<&HistoryEntry> = entries.iter().filter(|e| e.batch == *b).collect();
            let label = label(&es);
            Batch {
                batch: b,
                time: es.first().map_or("", |e| e.time.as_str()),
                label,
                items: es.len(),
                size: es.iter().filter(|e| e.ok).map(|e| e.size).sum(),
                restored: es
                    .iter()
                    .filter(|e| restored.contains(e.id.as_str()))
                    .count(),
                entries: es,
            }
        })
        .collect();
    let note = (skipped > 0).then(|| format!("{skipped} unreadable lines skipped"));
    if json {
        let errors: Vec<ErrorEntry> = note
            .into_iter()
            .map(|m| ErrorEntry::new("history", m))
            .collect();
        print_json_entries("history", &json!({ "batches": batches }), &errors)?;
        return Ok(ExitCode::SUCCESS);
    }
    let mut out = anstream::stdout().lock();
    if batches.is_empty() {
        writeln!(out, "no history yet")?;
    }
    for b in &batches {
        let mut line = format!(
            "{}  {}  {}  {} items  {}",
            b.batch,
            short_time(b.time),
            sanitise(&b.label),
            b.items,
            fmt_size(b.size)
        );
        if b.restored > 0 {
            line.push_str(&format!("  [restored {}]", b.restored));
        }
        writeln!(out, "{line}")?;
    }
    drop(out);
    if let Some(n) = note {
        writeln!(anstream::stderr().lock(), "warning: history: {n}")?;
    }
    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use super::short_time;

    #[test]
    fn short_time_trims_rfc3339() {
        assert_eq!(
            short_time("2026-11-08T10:00:00.123Z"),
            "2026-11-08T10:00:00"
        );
        assert_eq!(short_time("t"), "t");
    }
}
