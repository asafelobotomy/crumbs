//! Refusing early, choosing what to act on and dropping overlapping targets.

use super::*;

/// Exit 1 when some source failed, unless `--force` or `--dry-run` (a
/// preview only warns). `failures` has one entry per failed source: the
/// refusal shows their messages (and under `--json` includes them) first.
pub fn refuse_failed(failures: &[ErrorEntry], act: &ActFlags) -> Result<(), Stop> {
    if failures.is_empty() || act.force || act.dry_run {
        return Ok(());
    }
    let mut failed: Vec<&str> = failures.iter().map(|e| e.source.as_str()).collect();
    failed.sort_unstable();
    failed.dedup();
    let msg = format!(
        "refusing: some sources failed ({}); re-run with --force to continue",
        failed.join(", ")
    );
    Err(Stop::Refused {
        refusal: Refusal::new(1, msg),
        causes: failures.to_vec(),
    })
}

/// [`refuse_failed`] for the inventory's failed sources.
pub fn refuse_source_errors(inv: &Inventory, act: &ActFlags) -> Result<(), Stop> {
    let failures: Vec<ErrorEntry> = inv.errors.iter().map(ErrorEntry::from).collect();
    refuse_failed(&failures, act)
}

/// Reports why a build stopped: a refusal (after its causes), a cancelled
/// checklist, or an error for `main`.
pub fn stop(out: Out, stop: Stop) -> anyhow::Result<ExitCode> {
    match stop {
        Stop::Refused { refusal, causes } => out.fail_after(
            refusal.exit_code,
            &refusal.message,
            &refusal.details,
            &causes,
        ),
        Stop::Aborted => out.aborted(),
        Stop::Failed(e) => Err(e),
    }
}

/// Findings a plan may ever contain: a shared `mimeapps.list` edit is
/// never offered.
pub fn plannable(findings: Vec<Finding>) -> Vec<Finding> {
    findings
        .into_iter()
        .filter(|f| !(f.shared && f.kind == FindingKind::MimeAssoc))
        .collect()
}

/// Returns the findings to plan: the default ones, or with `-i` the ones
/// the user ticks. `None` when the checklist was cancelled.
pub fn choose(
    env: &Env,
    prompt_text: &str,
    findings: Vec<Finding>,
    interactive: bool,
    defaults: impl Fn(&Finding) -> bool,
) -> anyhow::Result<Option<Vec<Finding>>> {
    if !interactive {
        return Ok(Some(default_selection(findings, defaults)));
    }
    if findings.is_empty() {
        return Ok(Some(findings));
    }
    let items: Vec<String> = findings.iter().map(|f| checklist_item(env, f)).collect();
    let ticks: Vec<bool> = findings.iter().map(&defaults).collect();
    let Some(picked) = prompt::multi_select(prompt_text, &items, &ticks)? else {
        return Ok(None);
    };
    Ok(Some(
        findings
            .into_iter()
            .enumerate()
            .filter(|(i, _)| picked.contains(i))
            .map(|(_, f)| f)
            .collect(),
    ))
}

/// The findings a non-interactive run plans: the default ones. Never
/// prompts.
pub fn default_selection(
    findings: Vec<Finding>,
    defaults: impl Fn(&Finding) -> bool,
) -> Vec<Finding> {
    findings.into_iter().filter(|f| defaults(f)).collect()
}

/// One line of the `-i` checklist; a shared item and one deleted as root
/// say so.
pub(super) fn checklist_item(env: &Env, f: &Finding) -> String {
    let shared = if f.shared { "  (shared)" } else { "" };
    let root = if f.scope == Scope::SystemTree {
        "  (root)"
    } else {
        ""
    };
    format!(
        "{:<12} {:>10}  {}{shared}{root}",
        finding_kind(f.kind),
        render::finding_size(&f.size),
        render::shown_path(env, &f.path)
    )
}

pub struct Target {
    pub heading: String,
    /// `None` for orphans and `clean`.
    pub app: Option<App>,
    pub plan: Plan,
}

/// Makes every path planned once: an action whose path equals or lies
/// under a path an earlier target already plans is dropped. A second edit
/// of the same file is merged into the earlier edit instead, since the
/// first edit replaces the file and the second could never verify.
pub fn drop_overlaps(targets: &mut [Target]) {
    for i in 1..targets.len() {
        let (earlier, rest) = targets.split_at_mut(i);
        let Some(t) = rest.first_mut() else { continue };
        let mut kept: Vec<PlannedAction> = Vec::with_capacity(t.plan.actions.len());
        // Old index -> new index of the actions that stay.
        let mut new_index: Vec<Option<usize>> = Vec::with_capacity(t.plan.actions.len());
        // Old index -> (plan, action) of the earlier action that covers a
        // dropped one.
        let mut cover: Vec<Option<(usize, usize)>> = Vec::with_capacity(t.plan.actions.len());
        for mut pa in t.plan.actions.drain(..) {
            // A command has no filesystem path to overlap with.
            let path = pa.action.path().map(Path::to_path_buf);
            let covered_by = path.as_ref().and_then(|path| {
                earlier.iter().enumerate().find_map(|(ti, e)| {
                    e.plan
                        .actions
                        .iter()
                        .position(|a| a.action.path().is_some_and(|ep| path.starts_with(ep)))
                        .map(|ai| (ti, ai))
                })
            });
            cover.push(covered_by);
            if covered_by.is_none() {
                // A dependency dropped because an earlier plan covers it is
                // waited for there: the executor skips this action if that
                // cover failed.
                if let Some(d) = pa.depends_on {
                    match new_index.get(d).copied().flatten() {
                        Some(n) => pa.depends_on = Some(n),
                        None => {
                            pa.depends_on = None;
                            pa.depends_on_plan = cover.get(d).copied().flatten();
                        }
                    }
                }
                new_index.push(Some(kept.len()));
                kept.push(pa);
                continue;
            }
            new_index.push(None);
            if let Action::EditFile { remove_lines, .. } = &pa.action {
                let first = earlier
                    .iter_mut()
                    .flat_map(|e| e.plan.actions.iter_mut())
                    .map(|e| &mut e.action)
                    .find(|a| a.path() == path.as_deref());
                if let Some(Action::EditFile {
                    remove_lines: lines,
                    ..
                }) = first
                {
                    for l in remove_lines {
                        if !lines.contains(l) {
                            lines.push(l.clone());
                        }
                    }
                }
            }
        }
        t.plan.actions = kept;
        t.plan.total_size = t.plan.actions.iter().map(|a| a.size).sum();
    }
}
