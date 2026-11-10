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
mod helpers;
mod params;
pub(crate) use helpers::*;
pub(crate) use params::*;

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
                exclude: p.exclude,
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
mod tests;
