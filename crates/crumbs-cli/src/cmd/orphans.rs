use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::Args;
use crumbs_core::Env;
use crumbs_core::leftovers::{Finding, find_orphans, find_system_orphans};
use crumbs_core::plan::{PlanOptions, build_plan};
use serde_json::json;

use super::act::{
    ActFlags, Out, Target, apply, choose, plannable, precheck, refuse_source_errors, stop,
};
use super::build::{Built, Draft, Pick, Refusal, Stop, expand_home};
use super::context::{Context, print_notes};
use super::render::{finding_size, sanitise, shown_path};
use crate::output::{
    confidence, finding_kind, fmt_size, partial_failure, print_json_entries, print_table,
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

pub const USER_AND_SYSTEM: &str = "use either --user or --system";
const NEEDS_SELECT: &str = "orphans --clean needs --select <path>... or -i";

pub fn run(env: &Env, args: &OrphansArgs) -> anyhow::Result<ExitCode> {
    let act = &args.act;
    let out = Out {
        command: "orphans",
        json: act.json,
    };
    if args.user && args.system {
        return out.fail(2, USER_AND_SYSTEM);
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
fn select_path(env: &Env, s: &Path) -> Option<PathBuf> {
    let p = expand_home(env, s);
    p.is_absolute().then_some(p)
}

/// `orphans --clean`: only explicitly chosen orphans are ever planned.
fn clean(env: &Env, out: Out, args: &OrphansArgs) -> anyhow::Result<ExitCode> {
    let act = &args.act;
    if !act.interactive && args.select.is_empty() {
        return out.fail(2, NEEDS_SELECT);
    }
    if let Some(code) = precheck(out, act)? {
        return Ok(code);
    }
    let select: Vec<PathBuf> = args.select.iter().map(PathBuf::from).collect();
    let pick = if act.interactive {
        Pick::Checklist
    } else {
        Pick::Named(&select)
    };
    let Draft { built, cfg, .. } = match build_clean(env, args, pick) {
        Ok(draft) => draft,
        Err(s) => return stop(out, s),
    };
    apply(env, &cfg, &built.errors, out, &built.targets, act, &[])
}

/// Plans removing the orphans `pick` names (or the user ticks); nothing is
/// planned by default.
pub(crate) fn build_clean(
    env: &Env,
    args: &OrphansArgs,
    pick: Pick<PathBuf>,
) -> Result<Draft, Stop> {
    let act = &args.act;
    if args.user && args.system {
        return Err(Refusal::new(2, USER_AND_SYSTEM).into());
    }
    let select = match pick {
        Pick::Named(select) if !select.is_empty() => Some(select),
        Pick::Checklist => None,
        Pick::Named(_) | Pick::Defaults => return Err(Refusal::new(2, NEEDS_SELECT).into()),
    };
    let cx = Context::load(env)?;
    refuse_source_errors(&cx.inv, act)?;
    let orphans = if args.system {
        find_system_orphans(env, &cx.cfg, &cx.idx)
    } else {
        plannable(find_orphans(&cx.idx, args.show_low))
    };
    let selectable = orphans.clone();
    let mut wanted = Vec::new();
    let chosen = match select {
        None => choose(
            env,
            if args.system {
                "System orphans to remove"
            } else {
                "Orphans to remove"
            },
            orphans,
            true,
            |_| false,
        )?
        .ok_or(Stop::Aborted)?,
        Some(select) => {
            for s in select {
                let Some(p) = select_path(env, s) else {
                    let msg = format!("not an absolute path: {}", s.display());
                    return Err(Refusal::new(2, msg).into());
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
                    let msg = format!("not an orphan: {}{hint}", shown_path(env, p));
                    return Err(Refusal::new(2, msg).into());
                }
            }
            orphans
                .into_iter()
                .filter(|f| wanted.contains(&f.path))
                .collect()
        }
    };
    let opts = PlanOptions {
        permanent: act.permanent,
        ..PlanOptions::default()
    };
    let plan = build_plan(env, None, &chosen, &opts)?;
    let targets = vec![Target {
        heading: if args.system {
            "System orphans"
        } else {
            "Orphans"
        }
        .into(),
        app: None,
        plan,
    }];
    let mut cli_command = vec![
        "crumbs".to_string(),
        "orphans".to_string(),
        "--clean".to_string(),
    ];
    let flags = [
        (args.system, "--system"),
        (args.show_low, "--show-low"),
        (act.permanent, "--permanent"),
        (act.force, "--force"),
        (select.is_none(), "-i"),
    ];
    cli_command.extend(
        flags
            .iter()
            .filter(|(on, _)| *on)
            .map(|(_, f)| f.to_string()),
    );
    for p in &wanted {
        cli_command.push("--select".into());
        cli_command.push(p.to_string_lossy().into_owned());
    }
    let built = Built {
        targets,
        errors: cx.errors(),
        after_preview: Vec::new(),
        selectable,
        cli_command,
    };
    Ok(Draft {
        built,
        cfg: cx.cfg,
        held: Vec::new(),
    })
}

/// The orphans the listing shows, with the context they came from.
pub(crate) fn find_listed(
    env: &Env,
    args: &OrphansArgs,
) -> anyhow::Result<(Context, Vec<Finding>)> {
    let cx = Context::load(env)?;
    let orphans: Vec<Finding> = if args.system {
        find_system_orphans(env, &cx.cfg, &cx.idx)
    } else {
        find_orphans(&cx.idx, args.show_low)
    };
    Ok((cx, orphans))
}

fn list(env: &Env, args: &OrphansArgs, json: bool) -> anyhow::Result<ExitCode> {
    let (cx, orphans) = find_listed(env, args)?;
    let code = if partial_failure(&cx.inv) { 3 } else { 0 };
    if json {
        print_json_entries("orphans", &json!({ "orphans": orphans }), &cx.errors())?;
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
