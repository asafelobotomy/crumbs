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

use super::log::Log;

const INSTRUCTIONS: &str = "crumbs inspects installed Linux apps, their leftover files and \
reclaimable space. These tools only read; they change nothing.";

/// One server per connection. Clones share the log.
#[derive(Clone)]
pub(crate) struct Server {
    pub(super) env: Env,
    pub(super) log: Log,
    tool_router: ToolRouter<Server>,
}

impl Server {
    pub(crate) fn new(env: Env, log: Log) -> Server {
        Server {
            env,
            log,
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
