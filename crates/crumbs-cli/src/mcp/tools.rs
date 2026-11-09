//! The tools. Each read tool returns the CLI's `--json` `data` as its
//! structured content; a [`Refusal`] becomes a tool error.

use crumbs_core::bloat::Category;
use crumbs_core::{Env, SourceKind};
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, ContentBlock};
use rmcp::{ErrorData as McpError, Peer, RoleServer, tool, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use super::server::Server;
use crate::cmd::act::ActFlags;
use crate::cmd::apps::Sort;
use crate::cmd::build::{self, Refusal};
use crate::cmd::orphans::OrphansArgs;
use crate::cmd::render::sanitise;
use crate::cmd::scan::ScanArgs;
use crate::output::ErrorEntry;

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

/// Each `name` as `T`, or the usage error clap would give.
fn parse_all<T: std::str::FromStr<Err = String>>(names: &[String]) -> Result<Vec<T>, Refusal> {
    names
        .iter()
        .map(|n| n.parse().map_err(|e: String| Refusal::new(2, e)))
        .collect()
}

impl Server {
    /// Runs `f` off the async thread, then sends its warnings and returns
    /// its data.
    async fn read<F>(&self, peer: Peer<RoleServer>, f: F) -> Result<CallToolResult, McpError>
    where
        F: FnOnce(&Env) -> ReadOutcome + Send + 'static,
    {
        let env = self.env.clone();
        let outcome = tokio::task::spawn_blocking(move || f(&env))
            .await
            .map_err(|e| McpError::internal_error(format!("crumbs stopped: {e}"), None))?;
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
    /// Which sources and tools crumbs can use: the data of `crumbs doctor --json`.
    #[tool(annotations(read_only_hint = true))]
    async fn doctor(&self, peer: Peer<RoleServer>) -> Result<CallToolResult, McpError> {
        self.read(peer, build::doctor_data).await
    }

    /// The installed apps: the data of `crumbs apps list --json`.
    #[tool(annotations(read_only_hint = true))]
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

    /// One app and its leftovers (confidence, size, reason, scope): the
    /// data of `crumbs apps show <app> --json`.
    #[tool(annotations(read_only_hint = true))]
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

    /// Reclaimable space by category: the data of `crumbs scan --json`.
    /// Read-only.
    #[tool(annotations(read_only_hint = true))]
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

    /// Files that belong to no installed app: the data of `crumbs orphans
    /// --json`.
    #[tool(annotations(read_only_hint = true))]
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

    /// Past removals, newest batch first: the data of `crumbs history
    /// --json`.
    #[tool(annotations(read_only_hint = true))]
    async fn history(
        &self,
        Parameters(p): Parameters<HistoryParams>,
        peer: Peer<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        self.read(peer, move |env| build::history_data(env, p.limit))
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
    fn history_limit_defaults_to_50() {
        let p: HistoryParams = serde_json::from_value(json!({})).unwrap();
        assert_eq!(p.limit, 50);
    }
}
