//! `apply` in-process: the confirmation gate and what runs after it. Every
//! test runs on a fake root; privileged runs use the `/bin/sh` stand-ins.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;
use std::time::Duration;

use crumbs_core::Env;
use crumbs_core::history::{HistoryAction, HistoryEntry, read_history};
use crumbs_core::privilege::batch::{PrivilegedRun, PrivilegedRunner};
use crumbs_core::privilege::{Escalation, EscalationKind, Interaction};
use rmcp::model::{CallToolResponse, CallToolResult, InputResponses};
use serde_json::{Value, json};

use super::confirm::{CONFIRM_SUFFIX, EXPIRED_WHILE_ASKING, PLAN_NOT_FOUND};
use super::plans::{MAX_PLANS, PlanId, PlanPayload, StoredPlan, TOO_MANY_PLANS};
use super::server::Server;
use super::testing::{Answer, Asker, Connected, inky_with_system, test_server};
use crate::cmd::act::{NOTE_CHECK_MOUNTS, Privilege};

const CACHE: &str = "/home/user/.cache/inky";

fn history(env: &Env) -> Vec<HistoryEntry> {
    read_history(env).unwrap().0
}

fn text(r: &CallToolResult) -> String {
    r.content[0].as_text().unwrap().text.clone()
}

fn data(r: &CallToolResult) -> Value {
    r.structured_content.clone().unwrap_or(Value::Null)
}

/// Previews `inky`'s leftovers (plus `select`ed ones) and returns the id.
async fn preview<C: rmcp::ClientHandler + Clone>(c: &Connected<C>, select: &[&str]) -> String {
    let r = c
        .call(
            "preview_uninstall",
            json!({ "app": "inky", "leftovers_only": true, "select": select }),
        )
        .await;
    assert_ne!(r.is_error, Some(true), "{r:?}");
    data(&r)["plan_id"].as_str().expect("a plan").to_string()
}

async fn apply<C: rmcp::ClientHandler + Clone>(c: &Connected<C>, id: &str) -> CallToolResult {
    c.call("apply", json!({ "plan_id": id })).await
}

async fn connect(env: Env, answer: Answer) -> Connected<Asker> {
    Connected::start(env, Asker::new(answer)).await
}

fn stored(c: &Connected<Asker>, id: &str) -> bool {
    c.plans
        .lock()
        .unwrap()
        .peek(&PlanId::parse(id).unwrap())
        .is_some()
}

/// The key and state of an `InputRequiredResult`.
fn input_required(r: &CallToolResponse) -> (String, String, String) {
    let CallToolResponse::InputRequired(ir) = r else {
        panic!("not input_required: {r:?}");
    };
    let requests = ir.input_requests.as_ref().unwrap();
    assert_eq!(requests.len(), 1);
    let (key, request) = requests.iter().next().unwrap();
    let v = serde_json::to_value(request).unwrap();
    assert_eq!(v["method"], "elicitation/create", "{v}");
    let message = v["params"]["message"].as_str().unwrap().to_string();
    (key.clone(), ir.request_state.clone().unwrap(), message)
}

fn responses(key: &str, answer: Value) -> InputResponses {
    let mut r = InputResponses::new();
    r.insert(key.to_string(), answer);
    r
}

fn complete(r: CallToolResponse) -> CallToolResult {
    match r {
        CallToolResponse::Complete(r) => r,
        other => panic!("not complete: {other:?}"),
    }
}

/// Denies like a dismissed prompt, after writing on stderr.
struct DeniedLoudly;

impl PrivilegedRunner for DeniedLoudly {
    fn run(&self, _: &Escalation, _: &[u8]) -> Result<Vec<u8>, crumbs_core::CoreError> {
        Ok(Vec::new())
    }

    fn run_captured(
        &self,
        _: &Escalation,
        _: &[u8],
    ) -> Result<PrivilegedRun, crumbs_core::CoreError> {
        Ok(PrivilegedRun {
            stdout: Vec::new(),
            stderr: b"polkit: \x1b[31mnot authorized\n".to_vec(),
        })
    }

    fn interaction(&self) -> Interaction {
        Interaction::NoTerminal
    }
}

/// A server whose escalation exists and is always denied.
fn denying_server(env: Env) -> Server {
    let mut server = test_server(env, None);
    server.privilege = Arc::new(|_: &Env, _: bool| Privilege {
        esc: Some(Escalation {
            kind: EscalationKind::Run0,
            program: "/usr/bin/run0".into(),
            requires_tty: false,
        }),
        runner: Box::new(DeniedLoudly),
    });
    server
}

fn dummy_plan() -> StoredPlan {
    StoredPlan {
        payload: PlanPayload::Plans(Vec::new()),
        preview_text: String::new(),
        cli_command: Vec::new(),
        expires_at: std::time::SystemTime::now() + Duration::from_secs(600),
        confirm_key: String::new(),
        asked: false,
    }
}

mod apply_input;
mod mrtr_privileged;
mod preview_failed;
