use std::cmp::Reverse;
use std::io::Write;
use std::process::ExitCode;

use clap::{Subcommand, ValueEnum};
use crumbs_core::command::SystemRunner;
use crumbs_core::inventory::{Resolved, discover, find_app};
use crumbs_core::leftovers::{Finding, find_leftovers};
use crumbs_core::sources::ListOptions;
use crumbs_core::{App, Env, SourceKind};
use serde_json::json;

use super::context::{Context, load_cfg, print_notes};
use super::render::{sanitise, shown_path};
use crate::output::{
    ErrorEntry, confidence, finding_kind, fmt_size, partial_failure, print_json_entries,
    print_table,
};

#[derive(Subcommand)]
pub enum AppsCmd {
    /// List installed apps
    List {
        /// Only show apps from this source (repeatable)
        #[arg(long = "source", value_name = "SOURCE")]
        sources: Vec<SourceKind>,
        /// Include apps without a graphical launcher
        #[arg(long)]
        all: bool,
        /// Sort order
        #[arg(long, value_enum, default_value_t = Sort::Name)]
        sort: Sort,
        /// Machine-readable output
        #[arg(long)]
        json: bool,
    },
    /// Show details of one app
    Show {
        /// App id, name, desktop id or display name
        app: String,
        /// Include low-confidence leftovers
        #[arg(long)]
        show_low: bool,
        /// Machine-readable output
        #[arg(long)]
        json: bool,
    },
}

#[derive(Clone, Copy, ValueEnum)]
pub enum Sort {
    Name,
    Size,
}

pub fn run(cmd: AppsCmd, env: &Env) -> anyhow::Result<ExitCode> {
    match cmd {
        AppsCmd::List {
            sources,
            all,
            sort,
            json,
        } => list(env, &sources, all, sort, json),
        AppsCmd::Show {
            app,
            show_low,
            json,
        } => show(env, &app, show_low, json),
    }
}

fn list(
    env: &Env,
    sources: &[SourceKind],
    all: bool,
    sort: Sort,
    json: bool,
) -> anyhow::Result<ExitCode> {
    let (cfg, notes) = load_cfg(env)?;
    let mut inv = discover(
        env,
        &SystemRunner::new(),
        &ListOptions { include_all: all },
        &cfg,
    );
    if !sources.is_empty() {
        inv.apps.retain(|a| sources.contains(&a.source));
        // A source that was not asked for is none of the caller's business.
        inv.errors.retain(|e| sources.contains(&e.source));
    }
    if matches!(sort, Sort::Size) {
        // Stable: ties keep name order; unknown sizes sort last.
        inv.apps
            .sort_by_key(|a| (a.install_size.is_none(), Reverse(a.install_size)));
    }
    let code = if partial_failure(&inv) { 3 } else { 0 };

    if json {
        let mut errors: Vec<ErrorEntry> = inv.errors.iter().map(ErrorEntry::from).collect();
        errors.extend(notes);
        print_json_entries(
            "apps list",
            &json!({ "apps": inv.apps, "statuses": inv.statuses }),
            &errors,
        )?;
    } else {
        let rows: Vec<Vec<String>> = inv
            .apps
            .iter()
            .map(|a| {
                vec![
                    sanitise(&a.display_name),
                    a.version.clone().unwrap_or_else(|| "-".into()),
                    a.source.as_str().into(),
                    a.install_size.map_or_else(|| "-".into(), fmt_size),
                    sanitise(&a.id),
                ]
            })
            .collect();
        print_table(&["NAME", "VERSION", "SOURCE", "SIZE", "ID"], &rows)?;
        let mut out = anstream::stdout().lock();
        writeln!(out, "\n{} apps", inv.apps.len())?;
        let mut err = anstream::stderr().lock();
        for e in &inv.errors {
            writeln!(err, "warning: {}: {}", e.source.as_str(), e.message)?;
        }
        drop(err);
        print_notes(&notes)?;
    }
    Ok(ExitCode::from(code))
}

fn show(env: &Env, query: &str, show_low: bool, json: bool) -> anyhow::Result<ExitCode> {
    let cx = Context::load(env)?;
    let inv = &cx.inv;
    let mut errors: Vec<ErrorEntry> = inv.errors.iter().map(ErrorEntry::from).collect();
    errors.extend(cx.notes.iter().cloned());
    let mut err = anstream::stderr().lock();
    match find_app(inv, query) {
        Resolved::Found(app) => {
            let leftovers = find_leftovers(&cx.idx, app, show_low);
            if json {
                let data = json!({ "app": app, "leftovers": leftovers });
                print_json_entries("apps show", &data, &errors)?;
            } else {
                print_app(env, app)?;
                print_leftovers(env, &leftovers)?;
                drop(err);
                print_notes(&cx.notes)?;
            }
            Ok(ExitCode::from(if partial_failure(inv) { 3 } else { 0 }))
        }
        Resolved::NotFound => {
            writeln!(err, "error: no installed app matches '{query}'")?;
            if json {
                let data = json!({ "app": null, "query": query, "candidates": [] });
                print_json_entries("apps show", &data, &errors)?;
            }
            Ok(ExitCode::from(2))
        }
        Resolved::Ambiguous(candidates) => {
            writeln!(err, "error: '{query}' matches several apps:")?;
            if json {
                let data = json!({ "app": null, "query": query, "candidates": candidates });
                print_json_entries("apps show", &data, &errors)?;
            }
            for a in candidates {
                writeln!(
                    err,
                    "  {}  {}  {}",
                    sanitise(&a.id),
                    sanitise(&a.display_name),
                    a.source.as_str()
                )?;
            }
            Ok(ExitCode::from(2))
        }
    }
}

fn print_leftovers(env: &Env, leftovers: &[Finding]) -> anyhow::Result<()> {
    writeln!(anstream::stdout().lock(), "\nLeftovers")?;
    let rows: Vec<Vec<String>> = leftovers
        .iter()
        .map(|f| {
            vec![
                confidence(f.confidence),
                fmt_size(f.size.bytes),
                finding_kind(f.kind),
                shown_path(env, &f.path),
                if f.shared { "shared" } else { "" }.to_string(),
            ]
        })
        .collect();
    print_table(&["CONF", "SIZE", "KIND", "PATH", "NOTE"], &rows)?;
    let total: u64 = leftovers.iter().map(|f| f.size.bytes).sum();
    let preselected = leftovers.iter().filter(|f| f.preselected).count();
    writeln!(
        anstream::stdout().lock(),
        "\n{} leftovers, {} ({preselected} preselected)",
        leftovers.len(),
        fmt_size(total)
    )?;
    Ok(())
}

fn print_app(env: &Env, a: &App) -> anyhow::Result<()> {
    let list = |v: &[String]| {
        if v.is_empty() {
            "-".to_string()
        } else {
            v.join(", ")
        }
    };
    let rows = [
        ("Name", sanitise(&a.display_name)),
        ("ID", sanitise(&a.id)),
        ("Source", a.source.as_str().to_string()),
        ("Version", a.version.clone().unwrap_or_else(|| "-".into())),
        (
            "Installation",
            serde_json::to_value(a.installation)?
                .as_str()
                .unwrap_or("-")
                .to_string(),
        ),
        ("Size", a.install_size.map_or_else(|| "-".into(), fmt_size)),
        (
            "Location",
            a.location
                .as_ref()
                .map_or_else(|| "-".into(), |p| shown_path(env, p)),
        ),
        ("Desktop IDs", list(&a.desktop_ids)),
        ("Executables", list(&a.exec_names)),
        (
            "Description",
            a.description.clone().unwrap_or_else(|| "-".into()),
        ),
    ];
    let mut out = anstream::stdout().lock();
    for (k, v) in rows {
        writeln!(out, "{:<13} {v}", format!("{k}:"))?;
    }
    Ok(())
}
