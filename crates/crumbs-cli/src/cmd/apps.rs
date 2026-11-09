use std::cmp::Reverse;
use std::io::Write;
use std::process::ExitCode;

use clap::{Subcommand, ValueEnum};
use crumbs_core::command::SystemRunner;
use crumbs_core::inventory::{Inventory, Resolved, discover, find_app};
use crumbs_core::leftovers::{Finding, find_leftovers, find_system_leftovers};
use crumbs_core::safety::Scope;
use crumbs_core::sources::ListOptions;
use crumbs_core::{App, Env, SourceKind};
use serde_json::json;

use super::context::{Context, load_cfg, print_notes};
use super::render::{finding_size, sanitise, shown_path};
use crate::output::{
    ErrorEntry, confidence, finding_kind, fmt_size, partial_failure, print_json_entries,
    print_table, source_exit_code,
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

/// The apps `apps list` shows, with the config warnings.
pub(crate) struct Listed {
    pub inv: Inventory,
    pub notes: Vec<ErrorEntry>,
}

impl Listed {
    pub fn json_data(&self) -> serde_json::Value {
        json!({ "apps": self.inv.apps, "statuses": self.inv.statuses })
    }

    /// Failed sources, then notes.
    pub fn json_errors(&self) -> Vec<ErrorEntry> {
        let mut errors: Vec<ErrorEntry> = self.inv.errors.iter().map(ErrorEntry::from).collect();
        errors.extend(self.notes.iter().cloned());
        errors
    }
}

/// The installed apps, of `sources` only when any are given, in `sort`
/// order.
pub(crate) fn gather_list(
    env: &Env,
    sources: &[SourceKind],
    all: bool,
    sort: Sort,
) -> anyhow::Result<Listed> {
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
    Ok(Listed { inv, notes })
}

fn list(
    env: &Env,
    sources: &[SourceKind],
    all: bool,
    sort: Sort,
    json: bool,
) -> anyhow::Result<ExitCode> {
    let listed = gather_list(env, sources, all, sort)?;
    let inv = &listed.inv;
    let code = source_exit_code(&inv.statuses, &inv.errors);

    if json {
        print_json_entries("apps list", &listed.json_data(), &listed.json_errors())?;
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
        print_notes(&listed.notes)?;
    }
    Ok(ExitCode::from(code))
}

/// What `apps show` found for a query.
pub(crate) enum Shown {
    /// The app, and its leftovers (system ones too, as `uninstall` offers
    /// them).
    Found {
        app: Box<App>,
        leftovers: Vec<Finding>,
    },
    NotFound,
    Ambiguous(Vec<App>),
}

/// The app `query` names and its leftovers, with the context they came
/// from.
pub(crate) fn gather_show(
    env: &Env,
    query: &str,
    show_low: bool,
) -> anyhow::Result<(Context, Shown)> {
    let cx = Context::load(env)?;
    let shown = match find_app(&cx.inv, query) {
        Resolved::Found(app) => {
            let mut leftovers = find_leftovers(&cx.idx, app, show_low);
            leftovers.extend(find_system_leftovers(env, &cx.cfg, &cx.idx, app, show_low));
            Shown::Found {
                app: Box::new(app.clone()),
                leftovers,
            }
        }
        Resolved::NotFound => Shown::NotFound,
        Resolved::Ambiguous(candidates) => {
            Shown::Ambiguous(candidates.into_iter().cloned().collect())
        }
    };
    Ok((cx, shown))
}

fn show(env: &Env, query: &str, show_low: bool, json: bool) -> anyhow::Result<ExitCode> {
    let (cx, shown) = gather_show(env, query, show_low)?;
    let errors = cx.errors();
    let mut err = anstream::stderr().lock();
    match shown {
        Shown::Found { app, leftovers } => {
            if json {
                let data = json!({ "app": app, "leftovers": leftovers });
                print_json_entries("apps show", &data, &errors)?;
            } else {
                print_app(env, &app)?;
                print_leftovers(env, &leftovers)?;
                drop(err);
                print_notes(&cx.notes)?;
            }
            Ok(ExitCode::from(if partial_failure(&cx.inv) { 3 } else { 0 }))
        }
        Shown::NotFound => {
            writeln!(err, "error: no installed app matches '{query}'")?;
            if json {
                let data = json!({ "app": null, "query": query, "candidates": [] });
                print_json_entries("apps show", &data, &errors)?;
            }
            Ok(ExitCode::from(2))
        }
        Shown::Ambiguous(candidates) => {
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
                finding_size(&f.size),
                finding_kind(f.kind),
                shown_path(env, &f.path),
                match (f.shared, f.scope == Scope::SystemTree) {
                    (true, true) => "shared, root",
                    (true, false) => "shared",
                    (false, true) => "root",
                    (false, false) => "",
                }
                .to_string(),
            ]
        })
        .collect();
    print_table(&["CONF", "SIZE", "KIND", "PATH", "NOTE"], &rows)?;
    writeln!(
        anstream::stdout().lock(),
        "\n{}",
        leftovers_total(leftovers)
    )?;
    Ok(())
}

/// `N leftovers, <size> (M preselected)`; the size says `+ unknown` when
/// some row's size is unknown (shown as `-`).
fn leftovers_total(leftovers: &[Finding]) -> String {
    let total: u64 = leftovers.iter().map(|f| f.size.bytes).sum();
    let unknown = leftovers.iter().any(|f| finding_size(&f.size) == "-");
    let preselected = leftovers.iter().filter(|f| f.preselected).count();
    let size = if unknown {
        format!("{} + unknown", fmt_size(total))
    } else {
        fmt_size(total)
    };
    format!(
        "{} leftovers, {size} ({preselected} preselected)",
        leftovers.len()
    )
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
        ("Installation", sanitise(&a.installation.label())),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crumbs_core::leftovers::{Confidence, FindingKind};
    use crumbs_core::safety::Fingerprint;
    use crumbs_core::size::DiskUsage;

    fn finding(bytes: u64, errors: u64) -> Finding {
        Finding {
            path: "/opt/inky".into(),
            kind: FindingKind::SystemData,
            confidence: Confidence::Medium,
            shared: false,
            reason: String::new(),
            size: DiskUsage {
                bytes,
                files: 0,
                errors,
            },
            preselected: false,
            scope: Scope::SystemTree,
            fingerprint: Fingerprint::default(),
            mime_lines: Vec::new(),
        }
    }

    #[test]
    fn total_says_unknown_when_a_size_is() {
        assert_eq!(
            leftovers_total(&[finding(4096, 0)]),
            "1 leftovers, 4.0 KiB (0 preselected)"
        );
        assert_eq!(
            leftovers_total(&[finding(4096, 0), finding(0, 1)]),
            "2 leftovers, 4.0 KiB + unknown (0 preselected)"
        );
        // Part read, something counted: known.
        assert_eq!(
            leftovers_total(&[finding(4096, 1)]),
            "1 leftovers, 4.0 KiB (0 preselected)"
        );
    }
}
