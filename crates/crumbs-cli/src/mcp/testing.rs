//! The server's tests talk to it in-process: rmcp's client over
//! `tokio::io::duplex`. [`TestClient`] records what the server sends it;
//! tests that need more (answering elicitations) give it more behaviour.

// The recorded log messages are MCP logging, which rmcp marks deprecated
// (see `log.rs`).
#![allow(deprecated, reason = "the recorded messages are MCP logging")]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use crumbs_core::Env;
use rmcp::model::{CallToolRequestParams, CallToolResult, LoggingMessageNotificationParam};
use rmcp::service::{NotificationContext, RunningService};
use rmcp::{ClientHandler, RoleClient, ServiceExt};
use serde_json::Value;

use super::log::Log;
use super::plans::PlanStore;
use super::server::Server;

/// A client that records the server's log messages.
#[derive(Clone, Default)]
pub(crate) struct TestClient {
    pub logs: Arc<Mutex<Vec<LoggingMessageNotificationParam>>>,
}

impl ClientHandler for TestClient {
    async fn on_logging_message(
        &self,
        params: LoggingMessageNotificationParam,
        _context: NotificationContext<RoleClient>,
    ) {
        self.logs.lock().unwrap().push(params);
    }
}

/// A client connected to a fresh server on `env`.
pub(crate) struct Connected<C: ClientHandler = TestClient> {
    pub client: RunningService<RoleClient, C>,
    pub handler: C,
    /// The server's plan store.
    pub plans: Arc<Mutex<PlanStore>>,
}

impl<C: ClientHandler + Clone> Connected<C> {
    /// Starts a server on `env` (on the current runtime) and connects
    /// `handler` to it.
    pub(crate) async fn start(env: Env, handler: C) -> Connected<C> {
        Connected::start_server(Server::new(env, Log::default()), handler).await
    }

    /// Serves `server` and connects `handler` to it.
    pub(crate) async fn start_server(server: Server, handler: C) -> Connected<C> {
        let plans = server.plans.clone();
        let (server_io, client_io) = tokio::io::duplex(1 << 16);
        tokio::spawn(async move {
            let running = server.serve(server_io).await.expect("server starts");
            let _ = running.waiting().await;
        });
        let client = handler.clone().serve(client_io).await.expect("connects");
        Connected {
            client,
            handler,
            plans,
        }
    }

    /// Calls `tool` with `arguments` (a JSON object).
    pub(crate) async fn call(&self, tool: &'static str, arguments: Value) -> CallToolResult {
        let Value::Object(arguments) = arguments else {
            panic!("arguments must be an object: {arguments}");
        };
        self.client
            .call_tool(CallToolRequestParams::new(tool).with_arguments(arguments))
            .await
            .unwrap_or_else(|e| panic!("{tool}: {e}"))
    }

    /// Closes the connection.
    pub(crate) async fn close(self) {
        let _ = self.client.cancel().await;
    }
}

impl Connected<TestClient> {
    pub(crate) async fn new(env: Env) -> Connected<TestClient> {
        Connected::start(env, TestClient::default()).await
    }

    /// The warnings received so far, waiting up to a few seconds for at
    /// least one matching `want`: notifications are handled concurrently
    /// with responses.
    pub(crate) async fn warnings_with(&self, want: &str) -> Vec<String> {
        for _ in 0..200 {
            let got = self.warnings();
            if got.iter().any(|w| w.contains(want)) {
                return got;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        self.warnings()
    }

    fn warnings(&self) -> Vec<String> {
        self.handler
            .logs
            .lock()
            .unwrap()
            .iter()
            .filter(|m| m.level == rmcp::model::LoggingLevel::Warning)
            .filter_map(|m| m.data.as_str().map(str::to_string))
            .collect()
    }
}
