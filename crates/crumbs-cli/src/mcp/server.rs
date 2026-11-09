//! The MCP server: its state, what it tells clients about itself, and
//! the protocol requests that are not tool calls.

// `logging/setLevel` is part of MCP logging, which rmcp marks deprecated
// (see `log.rs`).
#![allow(deprecated, reason = "logging/setLevel belongs to MCP logging")]

use crumbs_core::Env;
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::model::{Implementation, ServerCapabilities, ServerConfig, SetLevelRequestParams};
use rmcp::service::{NotificationContext, RequestContext};
use rmcp::{ErrorData as McpError, RoleServer, ServerHandler, tool_handler};

use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::confirm;
use super::log::Log;
use super::plans::PlanStore;
use crate::cmd::act::Privilege;

const INSTRUCTIONS: &str = "crumbs inspects installed Linux apps, their leftover files and \
reclaimable space. The read tools change nothing, and so do the preview tools: \
each returns the exact plan and a plan_id. apply runs a previewed plan only \
after the user confirms it in the client.";

/// How `apply` gets administrator rights for `env`, when `needed`.
pub(crate) type PrivilegeFor = Arc<dyn Fn(&Env, bool) -> Privilege + Send + Sync>;

/// One server per connection. Clones share the log, the plans and the
/// apply lock.
#[derive(Clone)]
pub(crate) struct Server {
    pub(super) env: Env,
    pub(super) log: Log,
    /// The previewed plans. Never held across an await.
    pub(super) plans: Arc<Mutex<PlanStore>>,
    /// Held by an `apply` from its confirmation round through execution,
    /// so two runs never overlap.
    pub(super) apply_lock: Arc<tokio::sync::Mutex<()>>,
    /// How long `apply` waits for the user's answer.
    pub(super) confirm_timeout: Duration,
    pub(super) privilege: PrivilegeFor,
    tool_router: ToolRouter<Server>,
}

impl Server {
    pub(crate) fn new(env: Env, log: Log) -> Server {
        let test_ms = std::env::var("CRUMBS_TEST_CONFIRM_TIMEOUT_MS").ok();
        Server {
            confirm_timeout: confirm::timeout(&env.root, test_ms.as_deref()),
            env,
            log,
            plans: Arc::default(),
            apply_lock: Arc::default(),
            privilege: Arc::new(Privilege::for_server),
            tool_router: Server::tool_router(),
        }
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for Server {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_logging()
                .build(),
        )
        .with_server_info(Implementation::new("crumbs", env!("CARGO_PKG_VERSION")))
        .with_instructions(INSTRUCTIONS)
    }

    async fn set_level(
        &self,
        request: SetLevelRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<(), McpError> {
        self.log.set_level(request.level);
        Ok(())
    }

    async fn on_initialized(&self, context: NotificationContext<RoleServer>) {
        self.log.flush(&context.peer).await;
    }
}
