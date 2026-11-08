//! `crumbs clean`: scan the chosen categories, pick the items, plan them,
//! then the shared confirm-and-execute flow in [`super::act`].

use std::io::Write;
use std::process::ExitCode;

use clap::Args;
use crumbs_core::Env;
use crumbs_core::bloat::{BloatItem, Category, ScanContext, scan_all};
use crumbs_core::command::SystemRunner;
use crumbs_core::plan::{PlanOptions, build_clean_plan};
use serde_json::json;

use super::act::{ActFlags, Out, Target, apply, precheck, refuse_failed};
use super::context::{ScanSetup, print_notes};
use super::render::sanitise;
use super::scan::SYSTEM_UNAVAILABLE;
use crate::output::{ErrorEntry, fmt_size, print_json_entries};
use crate::prompt;

#[derive(Args, Debug)]
pub struct CleanArgs {
    /// Categories to clean, even if switched off in the config (default:
    /// every enabled category)
    #[arg(value_name = "CATEGORY")]
    pub categories: Vec<Category>,
    /// Clean your own files (the default)
    #[arg(long, conflicts_with = "system")]
    pub user: bool,
    /// Clean system-wide locations (not available yet)
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
    if args.system {
        return out.fail(2, SYSTEM_UNAVAILABLE);
    }
    if let Some(code) = precheck(out, act)? {
        return Ok(code);
    }
    let setup = ScanSetup::load(env)?;
    let named = !args.categories.is_empty();
    // Naming categories cleans exactly those, whatever the config says.
    let which: Vec<Category> = Category::ALL
        .into_iter()
        .filter(|c| {
            if named {
                args.categories.contains(c)
            } else {
                !setup.cfg.categories_off.contains(c)
            }
        })
        .collect();
    let cx = ScanContext {
        env,
        cfg: &setup.cfg,
        inv: &setup.inv,
        runner: &SystemRunner,
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
    if let Some(code) = refuse_failed(out, &errors, act)? {
        return Ok(code);
    }
    errors.extend(setup.notes.iter().cloned());

    let items: Vec<BloatItem> = report
        .categories
        .into_iter()
        .flat_map(|r| r.items)
        .collect();
    let defaults: Vec<bool> = items.iter().map(|i| preselect(i, named)).collect();
    // Items a scanner held back (a running app); the opt-in Flatpak
    // command is not one of them.
    let held: Vec<&str> = if act.interactive {
        Vec::new()
    } else {
        items
            .iter()
            .filter(|i| !i.preselected && i.category != Category::FlatpakUnused)
            .map(|i| i.note.as_deref().unwrap_or(&i.label))
            .collect()
    };
    let held: Vec<String> = held.into_iter().map(String::from).collect();
    let chosen: Vec<BloatItem> = if act.interactive && !items.is_empty() {
        let lines: Vec<String> = items.iter().map(checklist_line).collect();
        let Some(picked) = prompt::multi_select("Items to clean", &lines, &defaults)? else {
            return out.aborted();
        };
        items
            .into_iter()
            .enumerate()
            .filter(|(i, _)| picked.contains(i))
            .map(|(_, item)| item)
            .collect()
    } else {
        items
            .into_iter()
            .zip(defaults)
            .filter_map(|(item, on)| on.then_some(item))
            .collect()
    };
    if chosen.is_empty() {
        return nothing_to_clean(out, act, &held, &errors);
    }

    // Planned only after choosing: `depends_on` indexes the plan's actions.
    let opts = PlanOptions {
        permanent: act.permanent,
        ..PlanOptions::default()
    };
    let targets = [Target {
        heading: "Clean".into(),
        app: None,
        plan: build_clean_plan(env, &chosen, &opts)?,
    }];
    apply(env, &setup.cfg, &errors, out, &targets, act)
}

/// Whether an item starts ticked: the scanner's own default, except that
/// naming `flatpak-unused` opts in to its command. Items a scanner held
/// back (their app is running) stay unticked even in a named category.
fn preselect(item: &BloatItem, named: bool) -> bool {
    item.preselected || (named && item.category == Category::FlatpakUnused)
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
    fn naming_a_category_only_opts_in_to_flatpak() {
        let mut flatpak = item(false, Some("decided by flatpak"));
        flatpak.category = Category::FlatpakUnused;
        // Held back for any reason, whatever the note says.
        let held = item(false, Some("anything"));
        let unnoted = item(false, None);
        let plain = item(true, None);
        assert!(preselect(&flatpak, true));
        assert!(!preselect(&flatpak, false));
        assert!(!preselect(&held, true) && !preselect(&held, false));
        assert!(!preselect(&unnoted, true));
        assert!(preselect(&plain, true) && preselect(&plain, false));
    }
}
