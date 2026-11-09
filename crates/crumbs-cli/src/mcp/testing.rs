//! The server's tests talk to it in-process: rmcp's client over
//! `tokio::io::duplex`. [`TestClient`] records what the server sends it;
//! [`Asker`] also answers the server's confirmations as a test scripts.

// The recorded log messages are MCP logging, which rmcp marks deprecated
// (see `log.rs`).
#![allow(deprecated, reason = "the recorded messages are MCP logging")]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use crumbs_core::Env;
use crumbs_core::testing::FixtureTree;
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ClientCapabilities, ClientConfig,
    ElicitRequestParams, ElicitResult, ElicitationAction, InputResponses,
    LoggingMessageNotificationParam, ProtocolVersion,
};
use rmcp::service::{
    ClientLifecycleMode, ClientServiceExt, NotificationContext, RequestContext, RunningService,
};
use rmcp::{ClientHandler, ErrorData, RoleClient, ServiceExt};
use serde_json::{Value, json};

use super::log::Log;
use super::plans::PlanStore;
use super::server::Server;
use crate::cmd::act::Privilege;

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

/// How an [`Asker`] answers the server's confirmation.
#[derive(Clone, Debug)]
pub(crate) enum Answer {
    /// `accept` with this content.
    Accept(Value),
    /// `accept` with no content at all.
    AcceptEmpty,
    Decline,
    Cancel,
    /// Never answers.
    Never,
}

impl Answer {
    pub(crate) fn yes() -> Answer {
        Answer::Accept(json!({ "confirm": true }))
    }
}

/// A client that declares form elicitation and answers every one with
/// `answer`, recording each message it was asked.
#[derive(Clone)]
pub(crate) struct Asker {
    pub answer: Answer,
    pub asked: Arc<Mutex<Vec<String>>>,
    /// Declares the elicitation capability.
    pub capable: bool,
}

impl Asker {
    pub(crate) fn new(answer: Answer) -> Asker {
        Asker {
            answer,
            asked: Arc::default(),
            capable: true,
        }
    }

    /// A client that cannot show a form at all.
    pub(crate) fn incapable() -> Asker {
        Asker {
            capable: false,
            ..Asker::new(Answer::yes())
        }
    }

    pub(crate) fn asked(&self) -> Vec<String> {
        self.asked.lock().unwrap().clone()
    }
}

impl ClientHandler for Asker {
    async fn create_elicitation(
        &self,
        request: ElicitRequestParams,
        _context: RequestContext<RoleClient>,
    ) -> Result<ElicitResult, ErrorData> {
        if let ElicitRequestParams::FormElicitationParams { message, .. } = &request {
            self.asked.lock().unwrap().push(message.clone());
        }
        Ok(match &self.answer {
            Answer::Accept(content) => {
                ElicitResult::new(ElicitationAction::Accept).with_content(content.clone())
            }
            Answer::AcceptEmpty => ElicitResult::new(ElicitationAction::Accept),
            Answer::Decline => ElicitResult::new(ElicitationAction::Decline),
            Answer::Cancel => ElicitResult::new(ElicitationAction::Cancel),
            Answer::Never => std::future::pending().await,
        })
    }

    fn get_info(&self) -> ClientConfig {
        let mut info = ClientConfig::default();
        if self.capable {
            info.capabilities = ClientCapabilities::builder().enable_elicitation().build();
        }
        info
    }
}

/// A server on `env` for tests: its privileged runs use the `CRUMBS_TEST_PRIV_RUNNER`
/// stand-in `runner` (`None`: no way to get the rights), whatever the
/// process environment says.
pub(crate) fn test_server(env: Env, runner: Option<&'static str>) -> Server {
    let mut server = Server::new(env, Log::default());
    server.privilege =
        Arc::new(move |env: &Env, needed: bool| Privilege::for_server_with(env, needed, runner));
    server
}

/// A package `inky` with a cache of its own, plus unowned `/opt/inky` and
/// `/etc/inky` that pass the system-tree checks; `/` stands in for root.
pub(crate) fn inky_with_system() -> FixtureTree {
    let t = FixtureTree::new();
    t.os_release("manjaro", "arch", "Manjaro Linux");
    t.pacman_pkg("coreutils", "9.5-1", Some(2_000_000), &["usr/bin/ls"]);
    t.pacman_pkg("inky", "1.0-1", Some(1_000), &["usr/bin/inky"]);
    t.write_exec("/usr/bin/inky", b"");
    t.write("/home/user/.cache/inky/blob", b"cache");
    t.write("/opt/inky/bin/inky", b"x");
    t.write("/etc/inky/inky.conf", b"x");
    t.mkdir("etc");
    t.mkdir("opt");
    for rel in ["", "etc", "opt"] {
        std::fs::set_permissions(
            t.dir.path().join(rel),
            std::os::unix::fs::PermissionsExt::from_mode(0o755),
        )
        .unwrap();
    }
    t.write_exec(
        "/usr/bin/rm",
        b"#!/bin/sh\necho \"$@\" >> \"$(dirname \"$0\")/../../rm-args\"\nexec /usr/bin/rm \"$@\"\n",
    );
    t.write_exec("/usr/bin/sh", b"#!/bin/sh\nexec /bin/sh \"$@\"\n");
    t.write_exec(
        "/usr/bin/readlink",
        b"#!/bin/sh\nexec /usr/bin/readlink \"$@\"\n",
    );
    t.write(
        "/proc/self/mountinfo",
        b"36 35 98:0 /mnt1 / rw,noatime master:1 - ext3 /dev/root rw\n",
    );
    t
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
        Connected::start_server(test_server(env, None), handler).await
    }

    /// Serves `server` and connects `handler` to it with `initialize`.
    pub(crate) async fn start_server(server: Server, handler: C) -> Connected<C> {
        Connected::connect(server, handler, ClientLifecycleMode::Initialize).await
    }

    /// Serves `server` and connects `handler` on protocol 2026-07-28,
    /// which has no `initialize`: every request carries its own metadata.
    pub(crate) async fn start_modern(server: Server, handler: C) -> Connected<C> {
        let lifecycle = ClientLifecycleMode::Discover {
            preferred_versions: vec![ProtocolVersion::V_2026_07_28],
        };
        Connected::connect(server, handler, lifecycle).await
    }

    async fn connect(server: Server, handler: C, lifecycle: ClientLifecycleMode) -> Connected<C> {
        let plans = server.plans.clone();
        let (server_io, client_io) = tokio::io::duplex(1 << 16);
        tokio::spawn(async move {
            let running = server.serve(server_io).await.expect("server starts");
            let _ = running.waiting().await;
        });
        let client = handler
            .clone()
            .serve_with_lifecycle(client_io, lifecycle)
            .await
            .expect("connects");
        Connected {
            client,
            handler,
            plans,
        }
    }

    /// One `tools/call` round, without fulfilling any input request: an
    /// MRTR retry when `request_state` or `responses` is given.
    pub(crate) async fn call_once(
        &self,
        tool: &'static str,
        arguments: Value,
        request_state: Option<&str>,
        responses: Option<InputResponses>,
    ) -> CallToolResponse {
        let Value::Object(arguments) = arguments else {
            panic!("arguments must be an object: {arguments}");
        };
        let mut params = CallToolRequestParams::new(tool).with_arguments(arguments);
        if let Some(state) = request_state {
            params = params.with_request_state(state);
        }
        if let Some(responses) = responses {
            params = params.with_input_responses(responses);
        }
        self.client
            .call_tool_once(params)
            .await
            .unwrap_or_else(|e| panic!("{tool}: {e}"))
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
