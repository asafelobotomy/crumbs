use std::io::Write;
use std::process::ExitCode;

use crumbs_core::Env;
use crumbs_core::env::display_path;
use crumbs_core::leftovers::find_orphans;
use serde_json::json;

use super::context::{Context, print_notes};
use crate::output::{
    ErrorEntry, confidence, finding_kind, fmt_size, partial_failure, print_json_entries,
    print_table,
};

pub fn run(env: &Env, show_low: bool, json: bool) -> anyhow::Result<ExitCode> {
    let cx = Context::load(env)?;
    let orphans = find_orphans(&cx.idx, show_low);
    let code = if partial_failure(&cx.inv) { 3 } else { 0 };
    if json {
        let mut errors: Vec<ErrorEntry> = cx.inv.errors.iter().map(ErrorEntry::from).collect();
        errors.extend(cx.notes);
        print_json_entries("orphans", &json!({ "orphans": orphans }), &errors)?;
    } else {
        let rows: Vec<Vec<String>> = orphans
            .iter()
            .map(|f| {
                vec![
                    confidence(f.confidence),
                    fmt_size(f.size.bytes),
                    finding_kind(f.kind),
                    display_path(env, &f.path),
                    f.reason.clone(),
                ]
            })
            .collect();
        print_table(&["CONF", "SIZE", "KIND", "PATH", "REASON"], &rows)?;
        let total: u64 = orphans.iter().map(|f| f.size.bytes).sum();
        let mut out = anstream::stdout().lock();
        writeln!(out, "\n{} orphans, {}", orphans.len(), fmt_size(total))?;
        drop(out);
        for e in &cx.inv.errors {
            writeln!(
                anstream::stderr().lock(),
                "warning: {}: {}",
                e.source.as_str(),
                e.message
            )?;
        }
        print_notes(&cx.notes)?;
    }
    Ok(ExitCode::from(code))
}
