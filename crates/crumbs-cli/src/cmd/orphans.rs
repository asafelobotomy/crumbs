use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::Args;
use crumbs_core::Env;
use crumbs_core::leftovers::{Finding, find_orphans, find_system_orphans};
use crumbs_core::plan::{PlanOptions, build_plan};
use serde_json::json;

use super::act::{ActFlags, Out, Target, apply, choose, plannable, precheck, refuse_source_errors};
use super::context::{Context, print_notes};
use super::render::{finding_size, sanitise, shown_path};
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
    /// List (or with --clean, remove) unowned directories in /opt instead
    /// of per-user orphans; they are deleted with administrator rights
    #[arg(long)]
    pub system: bool,
    /// Per-user orphans (the default)
    #[arg(long)]
    pub user: bool,
    /// With --clean: an orphan to remove (repeatable)
    #[arg(long, value_name = "PATH", num_args = 1.., conflicts_with = "interactive")]
    pub select: Vec<String>,
    #[command(flatten)]
    pub act: ActFlags,
}

pub fn run(env: &Env, args: &OrphansArgs) -> anyhow::Result<ExitCode> {
    let act = &args.act;
    let out = Out {
        command: "orphans",
        json: act.json,
    };
    if args.user && args.system {
        return out.fail(2, "use either --user or --system");
    }
    if args.clean {
        return clean(env, out, args);
    }
    if !args.select.is_empty()
        || act.interactive
        || act.dry_run
        || act.yes
        || act.permanent
        || act.force
    {
        return out.fail(
            2,
            "--select, -i, --dry-run, --yes, --permanent and --force need --clean",
        );
    }
    list(env, args, act.json)
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
fn clean(env: &Env, out: Out, args: &OrphansArgs) -> anyhow::Result<ExitCode> {
    let act = &args.act;
    if !act.interactive && args.select.is_empty() {
        return out.fail(2, "orphans --clean needs --select <path>... or -i");
    }
    if let Some(code) = precheck(out, act)? {
        return Ok(code);
    }
    let cx = Context::load(env)?;
    if let Some(code) = refuse_source_errors(out, &cx.inv, act)? {
        return Ok(code);
    }
    let orphans = if args.system {
        find_system_orphans(env, &cx.cfg, &cx.idx)
    } else {
        plannable(find_orphans(&cx.idx, args.show_low))
    };
    let chosen = if act.interactive {
        match choose(
            env,
            if args.system {
                "System orphans to remove"
            } else {
                "Orphans to remove"
            },
            orphans,
            true,
            |_| false,
        )? {
            Some(c) => c,
            None => return out.aborted(),
        }
    } else {
        let mut wanted = Vec::new();
        for s in &args.select {
            let Some(p) = select_path(env, s) else {
                return out.fail(2, &format!("not an absolute path: {s}"));
            };
            if !wanted.contains(&p) {
                wanted.push(p);
            }
        }
        for p in &wanted {
            if !orphans.iter().any(|f| &f.path == p) {
                let hint = if !args.system
                    && !args.show_low
                    && find_orphans(&cx.idx, true).iter().any(|f| &f.path == p)
                {
                    " (it is low confidence; add --show-low)"
                } else {
                    ""
                };
                return out.fail(2, &format!("not an orphan: {}{hint}", shown_path(env, p)));
            }
        }
        orphans
            .into_iter()
            .filter(|f| wanted.contains(&f.path))
            .collect()
    };
    let opts = PlanOptions {
        permanent: act.permanent,
        ..PlanOptions::default()
    };
    let plan = build_plan(env, None, &chosen, &opts)?;
    let targets = [Target {
        heading: if args.system {
            "System orphans"
        } else {
            "Orphans"
        }
        .into(),
        app: None,
        plan,
    }];
    apply(env, &cx.cfg, &cx.errors(), out, &targets, act)
}

fn list(env: &Env, args: &OrphansArgs, json: bool) -> anyhow::Result<ExitCode> {
    let cx = Context::load(env)?;
    let orphans: Vec<Finding> = if args.system {
        find_system_orphans(env, &cx.cfg, &cx.idx)
    } else {
        find_orphans(&cx.idx, args.show_low)
    };
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
                    finding_size(&f.size),
                    finding_kind(f.kind),
                    shown_path(env, &f.path),
                    sanitise(&f.reason),
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
