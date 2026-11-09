//! The tools. Each read tool returns the CLI's `--json` `data` as its
//! structured content; a [`Refusal`] becomes a tool error.

use crumbs_core::bloat::Category;
use crumbs_core::{Env, SourceKind};
use rmcp::handler::server::tool::{InputResponses, RequestState};
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResponse, CallToolResult, ContentBlock};
use rmcp::service::RequestContext;
use rmcp::{ErrorData as McpError, Peer, RoleServer, tool, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use super::apply::Call;
use super::plans::{PLAN_TTL, PlanPayload, StoredPlan};
use super::server::Server;
use crate::cmd::act::{self, ActFlags};
use crate::cmd::apps::Sort;
use crate::cmd::build::{self, Built, Refusal};
use crate::cmd::clean::CleanArgs;
use crate::cmd::context::notes_text;
use crate::cmd::manual::is_privileged;
use crate::cmd::orphans::OrphansArgs;
use crate::cmd::render::sanitise;
use crate::cmd::restore::outcome_line;
use crate::cmd::scan::ScanArgs;
use crate::cmd::uninstall::UninstallArgs;
use crate::output::ErrorEntry;
use std::path::PathBuf;
use std::sync::MutexGuard;

/// What a read tool computes: the `--json` data and its warnings.
type ReadOutcome = Result<(Value, Vec<ErrorEntry>), Refusal>;

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct AppsListParams {
    /// Only apps from these sources: pacman, flatpak, appimage, unmanaged.
    #[serde(default)]
    pub source: Vec<String>,
    /// Include apps without a graphical launcher.
    #[serde(default)]
    pub all: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct AppShowParams {
    /// App id, name, desktop id or display name.
    pub app: String,
    /// Include low-confidence leftovers.
    #[serde(default)]
    pub show_low: bool,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ScanParams {
    /// Only these categories, even if switched off in the config.
    #[serde(default)]
    pub categories: Vec<String>,
    /// Scan your own files (the default).
    #[serde(default)]
    pub user: bool,
    /// Scan the system categories (read-only).
    #[serde(default)]
    pub system: bool,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct OrphansParams {
    /// Unowned directories in /opt instead of per-user orphans.
    #[serde(default)]
    pub system: bool,
    /// Per-user orphans (the default).
    #[serde(default)]
    pub user: bool,
    /// Include low-confidence findings.
    #[serde(default)]
    pub show_low: bool,
}

fn default_limit() -> usize {
    50
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct HistoryParams {
    /// Number of batches, newest first.
    #[serde(default = "default_limit")]
    pub limit: usize,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct PreviewUninstallParams {
    /// App id, name, desktop id or display name (exact match).
    pub app: String,
    /// Keep the app itself; remove only its leftovers.
    #[serde(default)]
    pub leftovers_only: bool,
    /// Keep data and settings; remove only caches.
    #[serde(default)]
    pub keep_data: bool,
    /// Include low-confidence leftovers among the findings `select` can name.
    #[serde(default)]
    pub show_low: bool,
    /// Plan exactly the leftovers at these paths, each one crumbs found
    /// for this app (`app_show` lists them). System leftovers are planned
    /// only when named here. Without it, crumbs' defaults apply.
    #[serde(default)]
    pub select: Vec<String>,
    /// Allow removing a package that is part of the uQ desktop (`part_of`
    /// is set on the app). Only when the user asked for that package by
    /// name: without it, such a removal is refused.
    #[serde(default)]
    pub desktop: bool,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct PreviewCleanParams {
    /// Only these categories, even if switched off in the config.
    #[serde(default)]
    pub categories: Vec<String>,
    /// Your own files (the default).
    #[serde(default)]
    pub user: bool,
    /// The system categories (needs administrator rights to apply).
    #[serde(default)]
    pub system: bool,
    /// Plan exactly these items (paths, or category ids for a category's
    /// items), each one `scan` found. Without it, the defaults apply.
    #[serde(default)]
    pub select: Vec<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct PreviewOrphansParams {
    /// The orphans to remove: absolute paths `orphans` found.
    pub paths: Vec<String>,
    /// Unowned directories in /opt instead of per-user orphans.
    #[serde(default)]
    pub system: bool,
    /// Include low-confidence findings among the orphans `paths` can name.
    #[serde(default)]
    pub show_low: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct ApplyParams {
    /// The `plan_id` a preview returned.
    pub plan_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct PreviewRestoreParams {
    /// A history entry id, a batch id, or a path to put back.
    pub entry: String,
}

/// A refusal as a tool error: the text is the message, the structured
/// content `{ exit_code, details }`, all sanitised.
pub(crate) fn refusal_result(r: &Refusal) -> CallToolResult {
    let details: Vec<String> = r.details.iter().map(|d| sanitise(d)).collect();
    let mut result =
        CallToolResult::structured_error(json!({ "exit_code": r.exit_code, "details": details }));
    result
        .content
        .insert(0, ContentBlock::text(sanitise(&r.message)));
    result
}

/// The error for a blocking task that never finished (it panicked or was
/// cancelled), its text sanitised.
pub(super) fn crumbs_stopped(e: &dyn std::fmt::Display) -> McpError {
    McpError::internal_error(sanitise(&format!("crumbs stopped: {e}")), None)
}

/// Flags for a preview: it only plans.
fn dry_run_flags() -> ActFlags {
    ActFlags {
        dry_run: true,
        json: true,
        ..ActFlags::default()
    }
}

/// Each `name` as `T`, or the usage error clap would give.
fn parse_all<T: std::str::FromStr<Err = String>>(names: &[String]) -> Result<Vec<T>, Refusal> {
    names
        .iter()
        .map(|n| n.parse().map_err(|e: String| Refusal::new(2, e)))
        .collect()
}

/// A preview before it is stored: what the result holds, and what
/// `apply` would run.
struct Preview {
    payload: PlanPayload,
    /// `dry_run_data`, an object.
    data: Value,
    preview_text: String,
    needs_admin: bool,
    cli_command: Vec<String>,
    errors: Vec<ErrorEntry>,
    /// A plan with nothing to do is shown, never stored.
    has_actions: bool,
}

impl Preview {
    fn of_plans(env: &Env, built: Built) -> Preview {
        let Built {
            targets,
            errors,
            after_preview,
            cli_command,
            ..
        } = built;
        let actions = || targets.iter().flat_map(|t| &t.plan.actions);
        Preview {
            data: act::dry_run_data(&targets),
            preview_text: act::preview_text(env, &targets, &after_preview, &errors),
            needs_admin: actions().any(is_privileged),
            has_actions: actions().next().is_some(),
            payload: PlanPayload::Plans(targets),
            cli_command,
            errors,
        }
    }
}

/// `argv` as one line for a shell: arguments with anything but plain
/// characters are single-quoted.
pub(super) fn shell_line(argv: &[String]) -> String {
    use std::ffi::OsStr;
    let plain = |a: &str| {
        !a.is_empty()
            && a.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_@%+=:,./-".contains(&b))
    };
    let words: Vec<String> = argv
        .iter()
        .map(|a| {
            if plain(a) {
                a.clone()
            } else {
                let quoted = crumbs_core::privilege::batch::sh_quote(OsStr::new(a));
                String::from_utf8_lossy(&quoted).into_owned()
            }
        })
        .collect();
    words.join(" ")
}

/// `select` as the builders take it: none means the CLI's defaults.
fn selection<T: From<String>>(select: Vec<String>) -> Option<Vec<T>> {
    (!select.is_empty()).then(|| select.into_iter().map(T::from).collect())
}

impl Server {
    pub(super) fn plans(&self) -> MutexGuard<'_, super::plans::PlanStore> {
        self.plans
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Drops the plans that have expired; every tool call starts here.
    pub(super) fn purge_plans(&self) {
        self.plans().purge_expired(self.now());
    }

    /// Runs `f` off the async thread, stores what it previews and returns
    /// the plan, its `plan_id` and the human preview.
    async fn preview<F>(&self, peer: Peer<RoleServer>, f: F) -> Result<CallToolResult, McpError>
    where
        F: FnOnce(&Env) -> Result<Preview, Refusal> + Send + 'static,
    {
        self.purge_plans();
        let env = self.env.clone();
        let outcome = tokio::task::spawn_blocking(move || f(&env))
            .await
            .map_err(|e| crumbs_stopped(&e))?;
        let preview = match outcome {
            Ok(p) => p,
            Err(refusal) => {
                self.log.flush(&peer).await;
                return Ok(refusal_result(&refusal));
            }
        };
        self.log.warn(&peer, &preview.errors).await;
        let Preview {
            payload,
            mut data,
            preview_text,
            needs_admin,
            cli_command,
            has_actions,
            ..
        } = preview;
        let line = shell_line(&cli_command);
        let mut plan_id = Value::Null;
        let mut expires = Value::Null;
        if has_actions {
            let expires_at = self.now() + PLAN_TTL;
            let Some(expires_text) = crumbs_core::history::rfc3339(expires_at) else {
                return Ok(refusal_result(&Refusal::new(
                    1,
                    "the plan's expiry time is out of range",
                )));
            };
            let stored = StoredPlan {
                payload,
                preview_text: preview_text.clone(),
                cli_command,
                expires_at,
                confirm_key: String::new(),
                asked: false,
            };
            match self.plans().insert(stored) {
                Ok(id) => {
                    plan_id = json!(id.as_str());
                    expires = json!(expires_text);
                }
                Err(e) => return Ok(refusal_result(&Refusal::new(1, e.to_string()))),
            }
        }
        data["cli_command"] = json!(line);
        data["plan_id"] = plan_id;
        data["expires_at"] = expires;
        data["needs_admin"] = json!(needs_admin);
        data["preview_text"] = json!(preview_text);
        Ok(CallToolResult::structured(data))
    }

    /// Runs `f` off the async thread, then sends its warnings and returns
    /// its data.
    async fn read<F>(&self, peer: Peer<RoleServer>, f: F) -> Result<CallToolResult, McpError>
    where
        F: FnOnce(&Env) -> ReadOutcome + Send + 'static,
    {
        self.purge_plans();
        let env = self.env.clone();
        let outcome = tokio::task::spawn_blocking(move || f(&env))
            .await
            .map_err(|e| crumbs_stopped(&e))?;
        Ok(match outcome {
            Ok((data, errors)) => {
                self.log.warn(&peer, &errors).await;
                CallToolResult::structured(data)
            }
            Err(refusal) => {
                self.log.flush(&peer).await;
                refusal_result(&refusal)
            }
        })
    }
}

#[tool_router(vis = "pub(super)")]
impl Server {
    #[tool(
        description = "Which sources and tools crumbs can use: the data of `crumbs doctor --json`.",
        annotations(read_only_hint = true)
    )]
    async fn doctor(&self, peer: Peer<RoleServer>) -> Result<CallToolResult, McpError> {
        self.read(peer, build::doctor_data).await
    }

    #[tool(
        description = "The installed apps: the data of `crumbs apps list --json`.",
        annotations(read_only_hint = true)
    )]
    async fn apps_list(
        &self,
        Parameters(p): Parameters<AppsListParams>,
        peer: Peer<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        self.read(peer, move |env| {
            let sources: Vec<SourceKind> = parse_all(&p.source)?;
            build::apps_list_data(env, &sources, p.all, Sort::Name)
        })
        .await
    }

    #[tool(
        description = "One app and its leftovers (confidence, size, reason, scope): the data of \
        `crumbs apps show <app> --json`.",
        annotations(read_only_hint = true)
    )]
    async fn app_show(
        &self,
        Parameters(p): Parameters<AppShowParams>,
        peer: Peer<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        self.read(peer, move |env| {
            build::app_show_data(env, &p.app, p.show_low)
        })
        .await
    }

    #[tool(
        description = "Reclaimable space by category: the data of `crumbs scan --json`. Read-only.",
        annotations(read_only_hint = true)
    )]
    async fn scan(
        &self,
        Parameters(p): Parameters<ScanParams>,
        peer: Peer<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        self.read(peer, move |env| {
            let args = ScanArgs {
                categories: parse_all::<Category>(&p.categories)?,
                user: p.user,
                system: p.system,
                details: false,
                json: true,
            };
            build::scan_data(env, &args)
        })
        .await
    }

    #[tool(
        description = "Files that belong to no installed app: the data of `crumbs orphans --json`.",
        annotations(read_only_hint = true)
    )]
    async fn orphans(
        &self,
        Parameters(p): Parameters<OrphansParams>,
        peer: Peer<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        self.read(peer, move |env| {
            let args = OrphansArgs {
                show_low: p.show_low,
                clean: false,
                system: p.system,
                user: p.user,
                select: Vec::new(),
                act: ActFlags {
                    json: true,
                    ..ActFlags::default()
                },
            };
            build::orphans_data(env, &args)
        })
        .await
    }

    #[tool(
        description = "Past removals, newest batch first: the data of `crumbs history --json`.",
        annotations(read_only_hint = true)
    )]
    async fn history(
        &self,
        Parameters(p): Parameters<HistoryParams>,
        peer: Peer<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        self.read(peer, move |env| build::history_data(env, p.limit))
            .await
    }

    #[tool(
        description = "What uninstalling an app (or only its leftovers) would remove: the plan, a \
        `plan_id` for `apply`, and the preview the CLI shows. Changes nothing.",
        annotations(read_only_hint = true)
    )]
    async fn preview_uninstall(
        &self,
        Parameters(p): Parameters<PreviewUninstallParams>,
        peer: Peer<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        self.preview(peer, move |env| {
            let args = UninstallArgs {
                apps: vec![p.app],
                leftovers_only: p.leftovers_only,
                keep_data: p.keep_data,
                show_low: p.show_low,
                desktop: p.desktop,
                act: dry_run_flags(),
            };
            let select: Option<Vec<PathBuf>> = selection(p.select);
            let built = build::uninstall(env, &args, select.as_deref())?;
            Ok(Preview::of_plans(env, built))
        })
        .await
    }

    #[tool(
        description = "What cleaning categories (caches, logs, ...) would remove: the plan, a \
        `plan_id` for `apply`, and the preview the CLI shows. Changes nothing.",
        annotations(read_only_hint = true)
    )]
    async fn preview_clean(
        &self,
        Parameters(p): Parameters<PreviewCleanParams>,
        peer: Peer<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        self.preview(peer, move |env| {
            let args = CleanArgs {
                categories: parse_all::<Category>(&p.categories)?,
                user: p.user,
                system: p.system,
                act: dry_run_flags(),
            };
            let select = selection::<String>(p.select);
            let built = build::clean(env, &args, select.as_deref())?;
            Ok(Preview::of_plans(env, built))
        })
        .await
    }

    #[tool(
        description = "What removing the named orphans would do: the plan, a `plan_id` for `apply`, \
        and the preview the CLI shows. Changes nothing.",
        annotations(read_only_hint = true)
    )]
    async fn preview_orphans(
        &self,
        Parameters(p): Parameters<PreviewOrphansParams>,
        peer: Peer<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        self.preview(peer, move |env| {
            let args = OrphansArgs {
                show_low: p.show_low,
                clean: true,
                system: p.system,
                user: false,
                select: p.paths.clone(),
                act: dry_run_flags(),
            };
            let paths: Vec<PathBuf> = p.paths.into_iter().map(PathBuf::from).collect();
            let built = build::orphans(env, &args, &paths)?;
            Ok(Preview::of_plans(env, built))
        })
        .await
    }

    #[tool(
        description = "What restoring a history entry, batch or path would put back: a `plan_id` for \
        `apply` and the list the CLI shows. Changes nothing.",
        annotations(read_only_hint = true)
    )]
    async fn preview_restore(
        &self,
        Parameters(p): Parameters<PreviewRestoreParams>,
        peer: Peer<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        self.preview(peer, move |env| {
            let selectors = vec![p.entry];
            let (built, outcomes) = build::restore(env, &selectors)?;
            let mut text: String = outcomes
                .iter()
                .map(|o| format!("{}\n", outcome_line(env, o, true)))
                .collect();
            text.push_str(&notes_text(&built.errors));
            let mut data = act::dry_run_data(&built.targets);
            data["outcomes"] =
                serde_json::to_value(&outcomes).map_err(|e| Refusal::new(1, e.to_string()))?;
            // The entries the dry run would restore, as shown.
            let entry_ids: Vec<String> = outcomes
                .iter()
                .filter(|o| o.ok && !o.entry_id.is_empty())
                .map(|o| o.entry_id.clone())
                .collect();
            Ok(Preview {
                data,
                preview_text: text,
                // What deleted as root is never restorable: a restore puts
                // back only the user's own files.
                needs_admin: false,
                has_actions: !entry_ids.is_empty(),
                payload: PlanPayload::Restore { entry_ids },
                cli_command: built.cli_command,
                errors: built.errors,
            })
        })
        .await
    }

    #[tool(
        description = "Runs a previewed plan, once, after the user confirms it in the client: the \
        client shows the user the plan's preview and asks \"Apply this plan?\". \
        Nothing runs without that yes. Returns the execution report, as the CLI's \
        `--json` output has it.",
        annotations(
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = false,
            open_world_hint = false
        )
    )]
    async fn apply(
        &self,
        Parameters(p): Parameters<ApplyParams>,
        context: RequestContext<RoleServer>,
        RequestState(request_state): RequestState,
        InputResponses(input_responses): InputResponses,
    ) -> Result<CallToolResponse, McpError> {
        self.apply_plan(Call {
            plan_id: p.plan_id,
            context,
            request_state,
            input_responses,
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refusal_text_and_details_are_sanitised() {
        let r = Refusal::new(2, "no app '\x1b[2Jx'").with_details(vec!["a\x07b".into()]);
        let result = refusal_result(&r);
        assert_eq!(result.is_error, Some(true));
        let text = result.content[0].as_text().unwrap().text.clone();
        assert!(!text.contains('\x1b'), "{text:?}");
        let structured = result.structured_content.unwrap();
        assert_eq!(structured["exit_code"], 2);
        let detail = structured["details"][0].as_str().unwrap();
        assert!(!detail.contains('\x07'), "{detail:?}");
    }

    #[test]
    fn crumbs_stopped_is_sanitised() {
        let e = crumbs_stopped(&"task \x1b[2Jpanicked\r");
        assert!(
            e.message.starts_with("crumbs stopped: task"),
            "{}",
            e.message
        );
        assert!(!e.message.contains('\x1b') && !e.message.contains('\r'));
    }

    #[test]
    fn history_limit_defaults_to_50() {
        let p: HistoryParams = serde_json::from_value(json!({})).unwrap();
        assert_eq!(p.limit, 50);
    }
}
