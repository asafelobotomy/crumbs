use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::Args;
use crumbs_core::Env;
use crumbs_core::env::display_path;
use crumbs_core::leftovers::find_orphans;
use crumbs_core::plan::{PlanOptions, build_plan};
use serde_json::json;

use super::context::{Context, print_notes};
use super::uninstall::{
    ActFlags, Target, apply, choose, plannable, precheck, refuse_source_errors,
};
use crate::output::{
    ErrorEntry, confidence, finding_kind, fmt_size, partial_failure, print_json_entries,
    print_table,
};

#[derive(Args, Debug)]
pub struct OrphansArgs {
    /// Include low-confidence findings
    #[arg(long)]
    pub show_low: bool,
    /// Remove orphans chosen with --select or -i
    #[arg(long)]
    pub clean: bool,
    /// With --clean: an orphan to remove (repeatable)
    #[arg(long, value_name = "PATH", num_args = 1.., conflicts_with = "interactive")]
    pub select: Vec<String>,
    #[command(flatten)]
    pub act: ActFlags,
}

pub fn run(env: &Env, args: &OrphansArgs) -> anyhow::Result<ExitCode> {
    let act = &args.act;
    if args.clean {
        return clean(env, args);
    }
    if !args.select.is_empty()
        || act.interactive
        || act.dry_run
        || act.yes
        || act.permanent
        || act.force
    {
        writeln!(
            anstream::stderr().lock(),
            "error: --select, -i, --dry-run, --yes, --permanent and --force need --clean"
        )?;
        return Ok(ExitCode::from(2));
    }
    list(env, args.show_low, act.json)
}

fn usage(msg: &str) -> anyhow::Result<ExitCode> {
    writeln!(anstream::stderr().lock(), "error: {msg}")?;
    Ok(ExitCode::from(2))
}

/// An in-system path from a `--select` value; a leading `~` is the home.
fn select_path(env: &Env, s: &str) -> Option<PathBuf> {
    let home = env.unsys(&env.home);
    let p = if s == "~" {
        home
    } else if let Some(rest) = s.strip_prefix("~/") {
        home.join(rest)
    } else {
        PathBuf::from(s)
    };
    p.is_absolute().then_some(p)
}

/// `orphans --clean`: only explicitly chosen orphans are ever planned.
fn clean(env: &Env, args: &OrphansArgs) -> anyhow::Result<ExitCode> {
    let act = &args.act;
    if !act.interactive && args.select.is_empty() {
        return usage("orphans --clean needs --select <path>... or -i");
    }
    if let Some(code) = precheck(act)? {
        return Ok(code);
    }
    let cx = Context::load(env)?;
    if let Some(code) = refuse_source_errors(&cx.inv, act.force)? {
        return Ok(code);
    }
    let orphans = plannable(find_orphans(&cx.idx, args.show_low));
    let chosen = if act.interactive {
        match choose(env, "Orphans to remove", orphans, true, |_| false)? {
            Some(c) => c,
            None => {
                writeln!(anstream::stderr().lock(), "aborted")?;
                return Ok(ExitCode::from(4));
            }
        }
    } else {
        let mut wanted = Vec::new();
        for s in &args.select {
            let Some(p) = select_path(env, s) else {
                return usage(&format!("not an absolute path: {s}"));
            };
            if !wanted.contains(&p) {
                wanted.push(p);
            }
        }
        for p in &wanted {
            if !orphans.iter().any(|f| &f.path == p) {
                let hint =
                    if !args.show_low && find_orphans(&cx.idx, true).iter().any(|f| &f.path == p) {
                        " (it is low confidence; add --show-low)"
                    } else {
                        ""
                    };
                return usage(&format!("not an orphan: {}{hint}", display_path(env, p)));
            }
        }
        orphans
            .into_iter()
            .filter(|f| wanted.contains(&f.path))
            .collect()
    };
    let opts = PlanOptions {
        permanent: act.permanent,
        keep_data: false,
    };
    let plan = build_plan(env, None, &chosen, &opts)?;
    let targets = [Target {
        heading: "Orphans".into(),
        app: None,
        plan,
    }];
    apply(env, &cx, "orphans", &targets, act)
}

fn list(env: &Env, show_low: bool, json: bool) -> anyhow::Result<ExitCode> {
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
