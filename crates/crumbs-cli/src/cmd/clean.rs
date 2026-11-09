//! `crumbs clean`: scan the chosen categories, pick the items, plan them,
//! then the shared confirm-and-execute flow in [`super::act`].

use std::io::Write;
use std::path::Path;
use std::process::ExitCode;

use clap::Args;
use crumbs_core::Env;
use crumbs_core::bloat::{BloatItem, Category, ScanContext, scan_all};
use crumbs_core::command::SystemRunner;
use crumbs_core::plan::{PlanOptions, build_clean_plan};
use serde_json::json;

use super::act::{ActFlags, Out, Target, apply, precheck, refuse_failed, stop};
use super::build::{Built, Draft, Pick, Refusal, Stop, expand_home, not_found};
use super::context::{ScanSetup, print_notes};
use super::render::sanitise;
use super::scan::{USER_AND_SYSTEM, cross_kind};
use crate::output::{ErrorEntry, fmt_size, print_json_entries};
use crate::prompt;

#[derive(Args, Debug)]
pub struct CleanArgs {
    /// Categories to clean, even if switched off in the config (default:
    /// every enabled category)
    #[arg(value_name = "CATEGORY")]
    pub categories: Vec<Category>,
    /// Clean your own files (the default)
    #[arg(long)]
    pub user: bool,
    /// Clean the system categories (asks for administrator rights once);
    /// naming a system category implies it
    #[arg(long)]
    pub system: bool,
    #[command(flatten)]
    pub act: ActFlags,
}

pub fn run(env: &Env, args: &CleanArgs) -> anyhow::Result<ExitCode> {
    let act = &args.act;
    let out = Out {
        command: "clean",
        json: act.json,
    };
    if let Some(refusal) = usage(args) {
        return out.fail(refusal.exit_code, &refusal.message);
    }
    if let Some(code) = precheck(out, act)? {
        return Ok(code);
    }
    let pick = if act.interactive {
        Pick::Checklist
    } else {
        Pick::Defaults
    };
    let Draft { built, cfg, held } = match build(env, args, pick) {
        Ok(draft) => draft,
        Err(s) => return stop(out, s),
    };
    if built.targets.is_empty() {
        return nothing_to_clean(out, act, &held, &built.errors);
    }
    apply(env, &cfg, &built.errors, out, &built.targets, act, &[])
}

/// `--user` with `--system`, or a named category of the other kind.
fn usage(args: &CleanArgs) -> Option<Refusal> {
    if args.user && args.system {
        return Some(Refusal::new(2, USER_AND_SYSTEM));
    }
    cross_kind(&args.categories, args.user, args.system).map(|msg| Refusal::new(2, msg))
}

/// Scans the chosen categories and plans the items `pick` chooses; no
/// target when none is chosen.
pub(crate) fn build(env: &Env, args: &CleanArgs, pick: Pick<String>) -> Result<Draft, Stop> {
    let act = &args.act;
    if let Some(refusal) = usage(args) {
        return Err(refusal.into());
    }
    let setup = ScanSetup::load(env)?;
    let named = !args.categories.is_empty();
    // Naming categories cleans exactly those, whatever the config says.
    // Without names, `--system` selects the system categories and no flag
    // the user ones; a named system category needs no flag.
    let which: Vec<Category> = Category::ALL
        .into_iter()
        .filter(|c| {
            if named {
                args.categories.contains(c)
            } else {
                c.system() == args.system && !setup.cfg.categories_off.contains(c)
            }
        })
        .collect();
    let cx = ScanContext {
        env,
        cfg: &setup.cfg,
        inv: &setup.inv,
        runner: &SystemRunner::new(),
        now: env.now(),
    };
    let report = scan_all(&cx, &which);

    // Failed sources, then failed scanners, then config notes.
    let mut errors: Vec<ErrorEntry> = setup.inv.errors.iter().map(ErrorEntry::from).collect();
    errors.extend(report.categories.iter().filter_map(|r| {
        r.error
            .as_ref()
            .map(|m| ErrorEntry::new(r.category.as_str(), m.clone()))
    }));
    refuse_failed(&errors, act)?;
    errors.extend(setup.notes.iter().cloned());

    let items: Vec<BloatItem> = report
        .categories
        .into_iter()
        .flat_map(|r| r.items)
        .collect();
    let defaults: Vec<bool> = items.iter().map(|i| preselect(i, named)).collect();
    // Items a scanner held back (a running app); the opt-in Flatpak
    // command and trash entries are not among them.
    let held: Vec<String> = if matches!(pick, Pick::Checklist) {
        Vec::new()
    } else {
        items
            .iter()
            .filter(|i| !i.preselected && !opt_in(i.category))
            .map(|i| i.note.as_deref().unwrap_or(&i.label).to_string())
            .collect()
    };
    let ticks: Vec<bool> = match pick {
        Pick::Checklist if !items.is_empty() => {
            let lines: Vec<String> = items.iter().map(checklist_line).collect();
            let picked =
                prompt::multi_select("Items to clean", &lines, &defaults)?.ok_or(Stop::Aborted)?;
            (0..items.len()).map(|i| picked.contains(&i)).collect()
        }
        Pick::Named(select) => named_ticks(env, &which, &items, select)?,
        Pick::Defaults | Pick::Checklist => defaults,
    };
    let chosen: Vec<BloatItem> = items
        .into_iter()
        .zip(ticks)
        .filter_map(|(item, on)| on.then_some(item))
        .collect();

    // Planned only after choosing: `depends_on` indexes the plan's actions.
    let targets = if chosen.is_empty() {
        Vec::new()
    } else {
        let opts = PlanOptions {
            permanent: act.permanent,
            ..PlanOptions::default()
        };
        vec![Target {
            heading: "Clean".into(),
            app: None,
            plan: build_clean_plan(env, &chosen, &opts)?,
        }]
    };
    let built = Built {
        targets,
        errors,
        after_preview: Vec::new(),
        cli_command: cli_command(args, &which, pick),
    };
    Ok(Draft {
        built,
        cfg: setup.cfg,
        held,
    })
}

/// Which of `items` the `select` values name: a category id (one of
/// `which`) stands for its items that would be ticked were it named, a
/// path for the item at it.
fn named_ticks(
    env: &Env,
    which: &[Category],
    items: &[BloatItem],
    select: &[String],
) -> Result<Vec<bool>, Refusal> {
    let mut ticks = vec![false; items.len()];
    for s in select {
        if let Some(c) = which.iter().find(|c| c.as_str() == s) {
            for (tick, item) in ticks.iter_mut().zip(items) {
                *tick |= item.category == *c && preselect(item, true);
            }
            continue;
        }
        let path = expand_home(env, Path::new(s));
        let at = items.iter().position(|i| i.path.as_ref() == Some(&path));
        match at.and_then(|i| ticks.get_mut(i)) {
            Some(tick) => *tick = true,
            None => return Err(not_found(s)),
        }
    }
    Ok(ticks)
}

/// The CLI command that plans the same. Categories named in `select` are
/// named; an item path needs the checklist (`-i`).
fn cli_command(args: &CleanArgs, which: &[Category], pick: Pick<String>) -> Vec<String> {
    let mut argv = vec!["crumbs".to_string(), "clean".to_string()];
    let mut interactive = matches!(pick, Pick::Checklist);
    match pick {
        Pick::Named(select) => {
            for s in select {
                match which.iter().find(|c| c.as_str() == s) {
                    Some(c) if !argv.iter().any(|a| a == c.as_str()) => {
                        argv.push(c.as_str().to_string());
                    }
                    Some(_) => {}
                    None => interactive = true,
                }
            }
            if interactive {
                // The checklist offers what the command scans.
                argv.truncate(2);
                argv.extend(args.categories.iter().map(|c| c.as_str().to_string()));
            }
        }
        Pick::Defaults | Pick::Checklist => {
            argv.extend(args.categories.iter().map(|c| c.as_str().to_string()));
        }
    }
    let flags = [
        (args.user, "--user"),
        (args.system, "--system"),
        (args.act.permanent, "--permanent"),
        (args.act.force, "--force"),
        (interactive, "-i"),
    ];
    argv.extend(
        flags
            .iter()
            .filter(|(on, _)| *on)
            .map(|(_, f)| f.to_string()),
    );
    argv
}

/// Categories whose items are never ticked unless the category is named:
/// the Flatpak commands, emptying the trash (it cannot be undone) and
/// removing orphaned packages (often optional dependencies people want).
fn opt_in(category: Category) -> bool {
    matches!(
        category,
        Category::FlatpakUnused
            | Category::Trash
            | Category::OrphanPkgs
            | Category::FlatpakUnusedSystem
    )
}

/// Whether an item starts ticked: the scanner's own default, except that
/// naming an [`opt_in`] category opts in to its items. Items a scanner held
/// back (their app is running) stay unticked even in a named category.
fn preselect(item: &BloatItem, named: bool) -> bool {
    item.preselected || (named && opt_in(item.category))
}

/// `CATEGORY  SIZE  LABEL  NOTE`, sanitised.
fn checklist_line(i: &BloatItem) -> String {
    let size = i.size.map_or_else(|| "?".to_string(), fmt_size);
    let mut s = format!(
        "{:<14} {size:>10}  {}",
        i.category.as_str(),
        sanitise(&i.label)
    );
    if let Some(n) = &i.note {
        s.push_str("  ");
        s.push_str(&sanitise(n));
    }
    s
}

/// No items chosen. The JSON matches an empty plan from `apply`: `report`
/// is an array aligned with `plans` (here empty), or `null` on a dry run.
fn nothing_to_clean(
    out: Out,
    act: &ActFlags,
    held: &[String],
    errors: &[ErrorEntry],
) -> anyhow::Result<ExitCode> {
    if out.json {
        let report = if act.dry_run { json!(null) } else { json!([]) };
        let data = json!({ "plans": [], "report": report });
        print_json_entries(out.command, &data, errors)?;
    } else {
        let mut line = "nothing to clean".to_string();
        if let Some(first) = held.first() {
            let n = held.len();
            let items = if n == 1 { "item" } else { "items" };
            line.push_str(&format!(" ({n} {items} held back: {})", sanitise(first)));
        }
        writeln!(anstream::stdout().lock(), "{line}")?;
        print_notes(errors)?;
    }
    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crumbs_core::bloat::ItemAction;
    use crumbs_core::safety::Scope;

    fn item(preselected: bool, note: Option<&str>) -> BloatItem {
        BloatItem {
            category: Category::Cache,
            path: None,
            label: String::new(),
            size: None,
            rebuildable: true,
            preselected,
            note: note.map(String::from),
            action: ItemAction::Trash,
            scope: Scope::Home,
            fingerprint: None,
            extra_paths: Vec::new(),
            app_id: None,
        }
    }

    #[test]
    fn naming_a_category_only_opts_in_to_the_opt_in_categories() {
        let mut flatpak = item(false, Some("decided by flatpak"));
        flatpak.category = Category::FlatpakUnused;
        let mut trash = item(false, None);
        trash.category = Category::Trash;
        assert!(preselect(&trash, true));
        assert!(!preselect(&trash, false));
        // Held back for any reason, whatever the note says.
        let held = item(false, Some("anything"));
        let unnoted = item(false, None);
        let plain = item(true, None);
        for category in [Category::OrphanPkgs, Category::FlatpakUnusedSystem] {
            let mut i = item(false, None);
            i.category = category;
            assert!(preselect(&i, true) && !preselect(&i, false));
        }
        assert!(preselect(&flatpak, true));
        assert!(!preselect(&flatpak, false));
        assert!(!preselect(&held, true) && !preselect(&held, false));
        assert!(!preselect(&unnoted, true));
        assert!(preselect(&plain, true) && preselect(&plain, false));
    }
}
