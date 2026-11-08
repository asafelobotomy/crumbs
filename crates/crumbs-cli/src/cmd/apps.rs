use std::cmp::Reverse;
use std::io::Write;
use std::process::ExitCode;

use clap::{Subcommand, ValueEnum};
use crumbs_core::command::SystemRunner;
use crumbs_core::config::Config;
use crumbs_core::inventory::{Inventory, Resolved, discover, find_app};
use crumbs_core::sources::ListOptions;
use crumbs_core::{App, Env, SourceKind};
use serde_json::json;

use crate::output::{fmt_size, partial_failure, print_json, print_table};

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
        AppsCmd::Show { app, json } => show(env, &app, json),
    }
}

fn list(
    env: &Env,
    sources: &[SourceKind],
    all: bool,
    sort: Sort,
    json: bool,
) -> anyhow::Result<ExitCode> {
    let mut inv = discover(
        env,
        &SystemRunner,
        &ListOptions { include_all: all },
        &Config::default(),
    );
    if !sources.is_empty() {
        inv.apps.retain(|a| sources.contains(&a.source));
    }
    if matches!(sort, Sort::Size) {
        // Stable: ties keep name order; unknown sizes sort last.
        inv.apps
            .sort_by_key(|a| (a.install_size.is_none(), Reverse(a.install_size)));
    }
    let code = if partial_failure(&inv) { 3 } else { 0 };

    if json {
        print_json(
            "apps list",
            &json!({ "apps": inv.apps, "statuses": inv.statuses }),
            &inv.errors,
        )?;
    } else {
        let rows: Vec<Vec<String>> = inv
            .apps
            .iter()
            .map(|a| {
                vec![
                    a.display_name.clone(),
                    a.version.clone().unwrap_or_else(|| "-".into()),
                    a.source.as_str().into(),
                    a.install_size.map_or_else(|| "-".into(), fmt_size),
                    a.id.clone(),
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
    }
    Ok(ExitCode::from(code))
}

fn show(env: &Env, query: &str, json: bool) -> anyhow::Result<ExitCode> {
    let inv: Inventory = discover(
        env,
        &SystemRunner,
        &ListOptions { include_all: true },
        &Config::default(),
    );
    let mut err = anstream::stderr().lock();
    match find_app(&inv, query) {
        Resolved::Found(app) => {
            if json {
                print_json("apps show", &json!({ "app": app }), &inv.errors)?;
            } else {
                print_app(app)?;
            }
            Ok(ExitCode::from(if partial_failure(&inv) { 3 } else { 0 }))
        }
        Resolved::NotFound => {
            writeln!(err, "error: no installed app matches '{query}'")?;
            if json {
                let data = json!({ "app": null, "query": query, "candidates": [] });
                print_json("apps show", &data, &inv.errors)?;
            }
            Ok(ExitCode::from(2))
        }
        Resolved::Ambiguous(candidates) => {
            writeln!(err, "error: '{query}' matches several apps:")?;
            if json {
                let data = json!({ "app": null, "query": query, "candidates": candidates });
                print_json("apps show", &data, &inv.errors)?;
            }
            for a in candidates {
                writeln!(err, "  {}  {}  {}", a.id, a.display_name, a.source.as_str())?;
            }
            Ok(ExitCode::from(2))
        }
    }
}

fn print_app(a: &App) -> anyhow::Result<()> {
    let list = |v: &[String]| {
        if v.is_empty() {
            "-".to_string()
        } else {
            v.join(", ")
        }
    };
    let rows = [
        ("Name", a.display_name.clone()),
        ("ID", a.id.clone()),
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
                .map_or_else(|| "-".into(), |p| p.display().to_string()),
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
