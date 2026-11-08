//! `crumbs scan`: find reclaimable space. Read-only.

use std::io::Write;
use std::process::ExitCode;

use clap::Args;
use crumbs_core::Env;
use crumbs_core::bloat::{BloatItem, Category, CategoryReport, ScanContext, ScanReport, scan_all};
use crumbs_core::command::SystemRunner;

use super::context::{ScanSetup, print_notes};
use super::render::sanitise;
use crate::output::{ErrorEntry, fmt_size, print_json_entries, print_table_with_details};

pub const USER_AND_SYSTEM: &str = "use either --user or --system";

/// A named category that contradicts `--user` or `--system`, as the usage
/// error to print; `None` when the flags and names agree.
pub fn cross_kind(categories: &[Category], user: bool, system: bool) -> Option<String> {
    let bad = categories
        .iter()
        .find(|c| (user && c.system()) || (system && !c.system()))?;
    let (flag, kind) = if bad.system() {
        ("--user", "system")
    } else {
        ("--system", "user")
    };
    Some(format!(
        "category '{}' is a {kind} category and cannot be used with {flag}",
        bad.as_str()
    ))
}
const OFF_NOTE: &str = "off (enable with --category or config)";

#[derive(Args, Debug)]
pub struct ScanArgs {
    /// Only these categories, even if switched off in the config (repeatable)
    #[arg(long = "category", value_name = "CATEGORY")]
    pub categories: Vec<Category>,
    /// Scan your own files (the default)
    #[arg(long)]
    pub user: bool,
    /// Scan the system categories (read-only; needs no administrator rights)
    #[arg(long)]
    pub system: bool,
    /// List every item under its category
    #[arg(long)]
    pub details: bool,
    /// Machine-readable output
    #[arg(long)]
    pub json: bool,
}

fn skipped(category: Category) -> CategoryReport {
    CategoryReport {
        category,
        items: Vec::new(),
        total: 0,
        unknown_sizes: 0,
        error: None,
        skipped: true,
        unavailable: None,
    }
}

pub fn run(env: &Env, args: &ScanArgs) -> anyhow::Result<ExitCode> {
    let usage = if args.user && args.system {
        Some(USER_AND_SYSTEM.to_string())
    } else {
        cross_kind(&args.categories, args.user, args.system)
    };
    if let Some(msg) = usage {
        writeln!(anstream::stderr().lock(), "error: {msg}")?;
        if args.json {
            let entry = [ErrorEntry::new("cli", msg)];
            print_json_entries("scan", &serde_json::Value::Null, &entry)?;
        }
        return Ok(ExitCode::from(2));
    }
    let setup = ScanSetup::load(env)?;
    let listed: Vec<Category> = if args.categories.is_empty() {
        // Without names: the user categories, or with `--system` the
        // system ones.
        Category::ALL
            .into_iter()
            .filter(|c| c.system() == args.system)
            .collect()
    } else {
        Category::ALL
            .into_iter()
            .filter(|c| args.categories.contains(c))
            .collect()
    };
    // Naming categories scans exactly those, whatever the config says.
    let enabled =
        |c: Category| !args.categories.is_empty() || !setup.cfg.categories_off.contains(&c);
    let which: Vec<Category> = listed.iter().copied().filter(|c| enabled(*c)).collect();
    let cx = ScanContext {
        env,
        cfg: &setup.cfg,
        inv: &setup.inv,
        runner: &SystemRunner::new(),
        now: env.now(),
    };
    let mut scanned = scan_all(&cx, &which).categories;
    let report = ScanReport {
        categories: listed
            .iter()
            .map(|&c| match scanned.iter().position(|r| r.category == c) {
                Some(i) => scanned.swap_remove(i),
                None => skipped(c),
            })
            .collect(),
    };
    let failed = report.categories.iter().any(|r| r.error.is_some());
    let code = ExitCode::from(if failed { 3 } else { 0 });

    if args.json {
        let mut errors: Vec<ErrorEntry> = report
            .categories
            .iter()
            .filter_map(|r| {
                r.error
                    .as_ref()
                    .map(|m| ErrorEntry::new(r.category.as_str(), m.clone()))
            })
            .collect();
        errors.extend(setup.inv.errors.iter().map(ErrorEntry::from));
        errors.extend(setup.notes);
        print_json_entries("scan", &report, &errors)?;
        return Ok(code);
    }

    print_human(&report, args.details)?;
    for e in &setup.inv.errors {
        writeln!(
            anstream::stderr().lock(),
            "warning: {}: {}",
            e.source.as_str(),
            sanitise(&e.message)
        )?;
    }
    print_notes(&setup.notes)?;
    Ok(code)
}

fn print_human(report: &ScanReport, details: bool) -> anyhow::Result<()> {
    let mut rows = Vec::new();
    let mut item_lines: Vec<Vec<String>> = Vec::new();
    for rep in &report.categories {
        let note = if rep.skipped {
            OFF_NOTE.to_string()
        } else if let Some(e) = &rep.error {
            sanitise(e)
        } else if let Some(why) = &rep.unavailable {
            format!("unavailable ({})", sanitise(why))
        } else if !rep.items.is_empty() && rep.items.iter().all(|i| i.rebuildable) {
            "rebuildable".to_string()
        } else {
            String::new()
        };
        let (items, size) = if rep.skipped || rep.error.is_some() || rep.unavailable.is_some() {
            ("-".to_string(), "-".to_string())
        } else {
            let size = match (rep.total, rep.unknown_sizes) {
                // Nothing known to add up: the size is unknown, not 0.
                (0, n) if n > 0 => "?".to_string(),
                (total, 0) => fmt_size(total),
                (total, _) => format!("{}+", fmt_size(total)),
            };
            (rep.items.len().to_string(), size)
        };
        rows.push(vec![rep.category.as_str().to_string(), items, size, note]);
        item_lines.push(if details {
            rep.items.iter().map(item_line).collect()
        } else {
            Vec::new()
        });
    }
    print_table_with_details(&["CATEGORY", "ITEMS", "SIZE", "NOTE"], &rows, &item_lines)?;
    let count: usize = report.categories.iter().map(|r| r.items.len()).sum();
    let total: u64 = report.categories.iter().map(|r| r.total).sum();
    let plus = if report.categories.iter().any(|r| r.unknown_sizes > 0) {
        "+"
    } else {
        ""
    };
    writeln!(
        anstream::stdout().lock(),
        "\nreclaimable: {}{plus} across {count} items",
        fmt_size(total)
    )?;
    Ok(())
}

/// `SIZE  LABEL  NOTE`; labels are already display strings (home as `~`).
fn item_line(i: &BloatItem) -> String {
    let size = i.size.map_or_else(|| "?".to_string(), fmt_size);
    let mut s = format!("{size}  {}", sanitise(&i.label));
    if let Some(n) = &i.note {
        s.push_str("  ");
        s.push_str(&sanitise(n));
    }
    s
}
