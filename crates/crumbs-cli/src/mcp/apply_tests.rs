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

#[tokio::test]
async fn apply_confirmed_runs_plan_and_writes_history() {
    let t = inky_with_system();
    let env = t.env();
    let c = connect(env.clone(), Answer::yes()).await;
    let r = c
        .call(
            "preview_uninstall",
            json!({ "app": "inky", "leftovers_only": true }),
        )
        .await;
    let preview_text = data(&r)["preview_text"].as_str().unwrap().to_string();
    let id = data(&r)["plan_id"].as_str().unwrap().to_string();

    let r = apply(&c, &id).await;
    assert_ne!(r.is_error, Some(true), "{r:?}");
    let d = data(&r);
    assert_eq!(d["applied"], true, "{d}");
    assert_eq!(d["exit_code"], 0, "{d}");
    let results = &d["report"][0]["results"];
    assert!(
        results.as_array().unwrap().iter().all(|r| r["ok"] == true),
        "{d}"
    );
    // The user was asked once, with the preview the agent was shown, one
    // blank line, then the question.
    assert!(
        preview_text.ends_with('\n') && !preview_text.ends_with("\n\n"),
        "{preview_text:?}"
    );
    assert_eq!(
        c.handler.asked(),
        [format!("{preview_text}\nApply this plan?")]
    );
    // In the trash, and in the history.
    assert!(!env.sys(CACHE).exists());
    let trash = env.sys("/home/user/.local/share/Trash/files/inky");
    assert!(trash.join("blob").exists(), "{}", trash.display());
    let h = history(&env);
    assert!(
        h.iter()
            .any(|e| e.action == HistoryAction::Trash && e.ok && e.path.ends_with(".cache/inky")),
        "{h:?}"
    );
    // The compact JSON text block carries the same data.
    assert_eq!(serde_json::from_str::<Value>(&text(&r)).unwrap(), d);
    // System leftovers were never selected.
    assert!(env.sys("/opt/inky/bin/inky").exists());
    c.close().await;
}

#[tokio::test]
async fn apply_without_confirmation_changes_nothing() {
    for (answer, reason) in [(Answer::Decline, "declined"), (Answer::Cancel, "cancelled")] {
        let t = inky_with_system();
        let env = t.env();
        let c = connect(env.clone(), answer).await;
        let id = preview(&c, &[]).await;
        let r = apply(&c, &id).await;
        assert_ne!(r.is_error, Some(true), "{r:?}");
        assert_eq!(data(&r), json!({ "applied": false, "reason": reason }));
        assert!(env.sys(CACHE).join("blob").exists());
        assert!(history(&env).is_empty());
        // The first round's no removes the plan.
        assert!(!stored(&c, &id));
        c.close().await;
    }

    // A client that cannot ask: refused before anything, the plan kept.
    let t = inky_with_system();
    let env = t.env();
    let c = Connected::start(env.clone(), Asker::incapable()).await;
    let id = preview(&c, &[]).await;
    let r = apply(&c, &id).await;
    assert_eq!(r.is_error, Some(true), "{r:?}");
    let msg = text(&r);
    assert_eq!(
        msg,
        "this client can't ask you to confirm; run this in a terminal instead: \
         crumbs uninstall inky --leftovers-only"
    );
    assert!(c.handler.asked().is_empty());
    assert!(env.sys(CACHE).join("blob").exists());
    assert!(history(&env).is_empty());
    assert!(stored(&c, &id));
    c.close().await;
}

#[tokio::test]
async fn apply_rejects_accept_without_confirm_true() {
    for answer in [
        Answer::Accept(json!({ "confirm": false })),
        Answer::AcceptEmpty,
        Answer::Accept(json!({ "confirm": "true" })),
        Answer::Accept(json!({})),
    ] {
        let t = inky_with_system();
        let env = t.env();
        let c = connect(env.clone(), answer.clone()).await;
        let id = preview(&c, &[]).await;
        let r = apply(&c, &id).await;
        assert_eq!(
            data(&r),
            json!({ "applied": false, "reason": "declined" }),
            "{answer:?}"
        );
        assert!(env.sys(CACHE).join("blob").exists(), "{answer:?}");
        assert!(history(&env).is_empty(), "{answer:?}");
        c.close().await;
    }
}

#[tokio::test]
async fn input_responses_never_confirm_before_2026() {
    // A pre-2026 client answers the elicitation with no; MRTR fields it
    // sends on its own count for nothing.
    let t = inky_with_system();
    let env = t.env();
    let c = connect(env.clone(), Answer::Decline).await;
    let id = preview(&c, &[]).await;
    let key = c
        .plans
        .lock()
        .unwrap()
        .peek(&PlanId::parse(&id).unwrap())
        .unwrap()
        .confirm_key
        .clone();
    let mut responses = InputResponses::new();
    responses.insert(
        key,
        json!({ "action": "accept", "content": { "confirm": true } }),
    );
    let r = c
        .call_once(
            "apply",
            json!({ "plan_id": id }),
            Some(&id),
            Some(responses),
        )
        .await;
    let CallToolResponse::Complete(r) = r else {
        panic!("{r:?}")
    };
    assert_eq!(data(&r), json!({ "applied": false, "reason": "declined" }));
    assert_eq!(c.handler.asked().len(), 1);
    assert!(env.sys(CACHE).join("blob").exists());
    c.close().await;
}

#[tokio::test]
async fn plan_id_is_single_use() {
    let t = inky_with_system();
    let c = connect(t.env(), Answer::yes()).await;
    let id = preview(&c, &[]).await;
    let r = apply(&c, &id).await;
    assert_eq!(data(&r)["applied"], true, "{r:?}");
    let r = apply(&c, &id).await;
    assert_eq!(r.is_error, Some(true), "{r:?}");
    assert_eq!(text(&r), PLAN_NOT_FOUND);
    // An id never issued, or not shaped like one: the same answer.
    for bogus in ["0".repeat(32), "nope".to_string()] {
        let r = apply(&c, &bogus).await;
        assert_eq!(text(&r), PLAN_NOT_FOUND);
    }
    // Only one confirmation was asked for.
    assert_eq!(c.handler.asked().len(), 1);
    c.close().await;
}

#[tokio::test]
async fn apply_refuses_swapped_target() {
    let t = inky_with_system();
    let env = t.env();
    let c = connect(env.clone(), Answer::yes()).await;
    let id = preview(&c, &[]).await;
    // Another directory at the same path: a new inode.
    std::fs::rename(env.sys(CACHE), env.sys("/home/user/.cache/inky.old")).unwrap();
    t.write("/home/user/.cache/inky/new", b"new");
    let r = apply(&c, &id).await;
    assert_eq!(r.is_error, Some(true), "{r:?}");
    let d = data(&r);
    // Its one action failed: nothing was applied, and the report says why.
    assert_eq!(d["applied"], false, "{d}");
    assert_eq!(d["exit_code"], 3, "{d}");
    let result = &d["report"][0]["results"][0];
    assert_eq!(result["ok"], false, "{d}");
    assert!(
        result["error"].as_str().is_some_and(|e| !e.is_empty()),
        "{d}"
    );
    assert!(env.sys("/home/user/.cache/inky/new").exists());
    c.close().await;
}

#[tokio::test]
async fn unanswered_confirmation_times_out_and_drops_plan() {
    let t = inky_with_system();
    let env = t.env();
    let mut server = test_server(env.clone(), None);
    server.confirm_timeout = Duration::from_millis(200);
    let c = Connected::start_server(server, Asker::new(Answer::Never)).await;
    let id = preview(&c, &[]).await;
    let r = apply(&c, &id).await;
    assert_eq!(data(&r), json!({ "applied": false, "reason": "timed out" }));
    // The lock was let go: another apply gets as far as asking.
    let second = preview(&c, &[]).await;
    let r = apply(&c, &second).await;
    assert_eq!(data(&r), json!({ "applied": false, "reason": "timed out" }));
    assert_eq!(c.handler.asked().len(), 2);
    let r = apply(&c, &id).await;
    assert_eq!(text(&r), PLAN_NOT_FOUND);
    assert!(env.sys(CACHE).join("blob").exists());
    assert!(history(&env).is_empty());
    c.close().await;
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

#[tokio::test]
async fn mrtr_flow_requires_matching_state() {
    let t = inky_with_system();
    let env = t.env();
    // The client would answer no if asked through `elicitation/create`:
    // on 2026-07-28 it never is.
    let c =
        Connected::start_modern(test_server(env.clone(), None), Asker::new(Answer::Decline)).await;
    let id = preview(&c, &[]).await;
    let other = preview(&c, &[]).await;
    let args = json!({ "plan_id": id });
    let first = c.call_once("apply", args.clone(), None, None).await;
    let (key, state, message) = input_required(&first);
    assert_eq!(state, id);
    assert!(message.ends_with(CONFIRM_SUFFIX), "{message}");
    assert!(stored(&c, &id), "the plan waits for the answer");
    let yes = json!({ "action": "accept", "content": { "confirm": true } });

    // Another plan's state, a state without answers, answers without a
    // state, or an answer under another key: nothing runs.
    let refused = [
        (Some(other.as_str()), Some(responses(&key, yes.clone()))),
        (Some(id.as_str()), None),
        (None, Some(responses(&key, yes.clone()))),
        (
            Some(id.as_str()),
            Some(responses("confirm-other", yes.clone())),
        ),
        (Some(id.as_str()), Some(responses(&id, yes.clone()))),
    ];
    for (state, answers) in refused {
        let r = complete(c.call_once("apply", args.clone(), state, answers).await);
        assert_eq!(r.is_error, Some(true), "{r:?}");
        assert!(env.sys(CACHE).join("blob").exists());
        assert!(history(&env).is_empty());
        assert!(stored(&c, &id));
    }
    // The other plan's own key does not confirm this one.
    let other_first = c
        .call_once("apply", json!({ "plan_id": other }), None, None)
        .await;
    let (other_key, _, _) = input_required(&other_first);
    assert_ne!(other_key, key);
    let r = complete(
        c.call_once(
            "apply",
            args.clone(),
            Some(&id),
            Some(responses(&other_key, yes.clone())),
        )
        .await,
    );
    assert_eq!(r.is_error, Some(true), "{r:?}");
    assert!(env.sys(CACHE).join("blob").exists());

    // The right state and the issued key, with confirm: true.
    let r = complete(
        c.call_once("apply", args.clone(), Some(&id), Some(responses(&key, yes)))
            .await,
    );
    assert_ne!(r.is_error, Some(true), "{r:?}");
    assert_eq!(data(&r)["applied"], true, "{r:?}");
    assert!(!env.sys(CACHE).exists());
    assert!(!stored(&c, &id));
    assert!(c.handler.asked().is_empty());
    c.close().await;
}

#[tokio::test]
async fn mrtr_no_and_auto_driven_yes() {
    let t = inky_with_system();
    let env = t.env();
    let c =
        Connected::start_modern(test_server(env.clone(), None), Asker::new(Answer::yes())).await;
    // A no in the final round removes the plan.
    let id = preview(&c, &[]).await;
    let args = json!({ "plan_id": id });
    let (key, _, _) = input_required(&c.call_once("apply", args.clone(), None, None).await);
    let no = json!({ "action": "accept", "content": { "confirm": false } });
    let r = complete(
        c.call_once("apply", args.clone(), Some(&id), Some(responses(&key, no)))
            .await,
    );
    assert_eq!(data(&r), json!({ "applied": false, "reason": "declined" }));
    assert!(!stored(&c, &id));
    assert!(env.sys(CACHE).join("blob").exists());
    // rmcp's client drives the rounds itself, asking its handler.
    let id = preview(&c, &[]).await;
    let r = apply(&c, &id).await;
    assert_eq!(data(&r)["applied"], true, "{r:?}");
    assert_eq!(c.handler.asked().len(), 1);
    assert!(!env.sys(CACHE).exists());
    c.close().await;
}

#[tokio::test]
async fn mrtr_plan_expires_while_waiting() {
    let t = inky_with_system();
    let env = t.env();
    let server = test_server(env.clone(), None);
    let c = Connected::start_modern(server.clone(), Asker::new(Answer::yes())).await;
    let id = preview(&c, &[]).await;
    let args = json!({ "plan_id": id });
    let (key, _, _) = input_required(&c.call_once("apply", args.clone(), None, None).await);
    let mut late = server;
    late.env.set_clock_shift(Duration::from_secs(601));
    let c2 = Connected::start_modern(late, Asker::new(Answer::yes())).await;
    let yes = json!({ "action": "accept", "content": { "confirm": true } });
    let r = complete(
        c2.call_once("apply", args, Some(&id), Some(responses(&key, yes)))
            .await,
    );
    assert_eq!(text(&r), PLAN_NOT_FOUND);
    assert!(env.sys(CACHE).join("blob").exists());
    c.close().await;
    c2.close().await;
}

#[tokio::test]
async fn mrtr_client_without_elicitation_is_refused() {
    let t = inky_with_system();
    let env = t.env();
    let c = Connected::start_modern(test_server(env.clone(), None), Asker::incapable()).await;
    let id = preview(&c, &[]).await;
    let r = complete(
        c.call_once("apply", json!({ "plan_id": id }), None, None)
            .await,
    );
    assert_eq!(r.is_error, Some(true), "{r:?}");
    assert!(text(&r).contains("crumbs uninstall inky"), "{r:?}");
    assert!(stored(&c, &id));
    c.close().await;
}

#[tokio::test]
async fn privileged_plan_without_rights_returns_manual_commands() {
    let t = inky_with_system();
    let env = t.env();
    let c = connect(env.clone(), Answer::yes()).await;
    let id = preview(&c, &[CACHE, "/opt/inky"]).await;
    let r = apply(&c, &id).await;
    assert_ne!(r.is_error, Some(true), "{r:?}");
    let d = data(&r);
    assert_eq!(d["applied"], false, "{d}");
    assert_eq!(d["exit_code"], 5, "{d}");
    let manual = d["manual_commands"].as_array().unwrap();
    assert!(
        manual
            .iter()
            .any(|m| m.as_str().unwrap().contains("'-rf' '--one-file-system'")),
        "{d}"
    );
    assert_eq!(d["note"], NOTE_CHECK_MOUNTS, "{d}");
    // Nothing ran: not even the user's own files.
    assert!(env.sys("/opt/inky/bin/inky").exists());
    assert!(env.sys(CACHE).join("blob").exists());
    c.close().await;
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

#[tokio::test]
async fn denied_escalation_of_a_system_only_plan_applies_nothing() {
    let t = inky_with_system();
    let env = t.env();
    let c = Connected::start_server(denying_server(env.clone()), Asker::new(Answer::yes())).await;
    let id = preview(&c, &["/opt/inky"]).await;
    let r = apply(&c, &id).await;
    assert_ne!(r.is_error, Some(true), "{r:?}");
    let d = data(&r);
    assert_eq!(d["applied"], false, "{d}");
    assert_eq!(d["exit_code"], 5, "{d}");
    assert!(!d["manual_commands"].as_array().unwrap().is_empty(), "{d}");
    assert_eq!(d["note"], NOTE_CHECK_MOUNTS, "{d}");
    assert!(
        d["escalation_output"]
            .as_str()
            .unwrap()
            .contains("not authorized"),
        "{d}"
    );
    // Nothing ran, so there is no report.
    assert!(d.get("report").is_none(), "{d}");
    assert!(env.sys("/opt/inky/bin/inky").exists());
    assert!(env.sys(CACHE).join("blob").exists());
    c.close().await;
}

#[tokio::test]
async fn denied_escalation_returns_its_output_sanitised() {
    let t = inky_with_system();
    let env = t.env();
    let c = Connected::start_server(denying_server(env.clone()), Asker::new(Answer::yes())).await;
    let id = preview(&c, &[CACHE, "/opt/inky"]).await;
    let r = apply(&c, &id).await;
    let d = data(&r);
    assert_eq!(d["exit_code"], 5, "{d}");
    // The user's own files were handled before the prompt: something ran.
    assert_eq!(d["applied"], true, "{d}");
    assert_eq!(d["note"], NOTE_CHECK_MOUNTS, "{d}");
    let output = d["escalation_output"].as_str().unwrap();
    assert!(output.contains("not authorized"), "{output:?}");
    assert!(!output.contains('\x1b'), "{output:?}");
    assert!(!d["manual_commands"].as_array().unwrap().is_empty(), "{d}");
    // The user's own files went; the report says what ran.
    assert!(!env.sys(CACHE).exists());
    assert!(!d["report"].is_null(), "{d}");
    assert!(env.sys("/opt/inky/bin/inky").exists());
    c.close().await;
}

#[tokio::test]
async fn privileged_plan_runs_in_one_batch_without_a_terminal() {
    let t = inky_with_system();
    let env = t.env();
    let c = Connected::start_server(
        test_server(env.clone(), Some("sh")),
        Asker::new(Answer::yes()),
    )
    .await;
    let id = preview(&c, &[CACHE, "/opt/inky", "/etc/inky"]).await;
    let r = apply(&c, &id).await;
    assert_ne!(r.is_error, Some(true), "{r:?}");
    let d = data(&r);
    assert_eq!(d["exit_code"], 0, "{d}");
    assert!(!env.sys("/opt/inky").exists());
    assert!(!env.sys("/etc/inky").exists());
    assert!(!env.sys(CACHE).exists());
    let h = history(&env);
    assert!(
        h.iter()
            .filter(|e| e.action == HistoryAction::Delete)
            .all(|e| e.ok),
        "{h:?}"
    );
    c.close().await;
}

#[tokio::test]
async fn restore_applies_exactly_the_previewed_entries() {
    let t = inky_with_system();
    let env = t.env();
    let c = connect(env.clone(), Answer::yes()).await;
    // Trash the cache once.
    let id = preview(&c, &[]).await;
    assert_eq!(data(&apply(&c, &id).await)["applied"], true);
    let first = history(&env)
        .into_iter()
        .find(|e| e.action == HistoryAction::Trash && e.path.ends_with(".cache/inky"))
        .unwrap();
    // Preview putting it back, by path.
    let r = c.call("preview_restore", json!({ "entry": CACHE })).await;
    assert_ne!(r.is_error, Some(true), "{r:?}");
    assert_eq!(data(&r)["needs_admin"], false);
    let restore_id = data(&r)["plan_id"].as_str().unwrap().to_string();
    {
        let plans = c.plans.lock().unwrap();
        let plan = plans.peek(&PlanId::parse(&restore_id).unwrap()).unwrap();
        let PlanPayload::Restore { entry_ids } = &plan.payload else {
            panic!("not a restore");
        };
        assert_eq!(entry_ids, std::slice::from_ref(&first.id));
    }
    // A newer trash entry for the same path, before the restore runs.
    t.write("/home/user/.cache/inky/blob2", b"second");
    let id = preview(&c, &[]).await;
    assert_eq!(data(&apply(&c, &id).await)["applied"], true);
    assert!(!env.sys(CACHE).exists());

    let r = apply(&c, &restore_id).await;
    assert_ne!(r.is_error, Some(true), "{r:?}");
    let d = data(&r);
    assert_eq!(d["applied"], true, "{d}");
    assert_eq!(d["exit_code"], 0, "{d}");
    // The first trashing came back, not the newer one.
    assert!(env.sys(CACHE).join("blob").exists());
    assert!(!env.sys(CACHE).join("blob2").exists());
    let restores: Vec<HistoryEntry> = history(&env)
        .into_iter()
        .filter(|e| e.action == HistoryAction::Restore)
        .collect();
    assert_eq!(restores.len(), 1, "{restores:?}");
    assert_eq!(restores[0].restores.as_deref(), Some(first.id.as_str()));
    c.close().await;
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

#[tokio::test]
async fn preview_past_the_cap_is_refused() {
    let t = inky_with_system();
    let c = connect(t.env(), Answer::yes()).await;
    {
        let mut plans = c.plans.lock().unwrap();
        for _ in 0..MAX_PLANS {
            plans.insert(dummy_plan()).unwrap();
        }
    }
    let r = c
        .call(
            "preview_uninstall",
            json!({ "app": "inky", "leftovers_only": true }),
        )
        .await;
    assert_eq!(r.is_error, Some(true), "{r:?}");
    assert_eq!(text(&r), TOO_MANY_PLANS);
    assert_eq!(c.plans.lock().unwrap().len(), MAX_PLANS);
    c.close().await;
}

#[tokio::test]
async fn preview_fails_when_the_expiry_is_out_of_range() {
    let t = inky_with_system();
    let mut server: Server = test_server(t.env(), None);
    server
        .env
        .set_clock_shift(Duration::from_secs(300_000_000_000));
    let c = Connected::start_server(server, Asker::new(Answer::yes())).await;
    let r = c
        .call(
            "preview_uninstall",
            json!({ "app": "inky", "leftovers_only": true }),
        )
        .await;
    assert_eq!(r.is_error, Some(true), "{r:?}");
    assert!(c.plans.lock().unwrap().is_empty());
    c.close().await;
}

#[tokio::test]
async fn mrtr_answer_before_the_question_is_refused() {
    let t = inky_with_system();
    let env = t.env();
    let c =
        Connected::start_modern(test_server(env.clone(), None), Asker::new(Answer::yes())).await;
    let id = preview(&c, &[]).await;
    // The real key, sent before the server ever asked.
    let key = c
        .plans
        .lock()
        .unwrap()
        .peek(&PlanId::parse(&id).unwrap())
        .unwrap()
        .confirm_key
        .clone();
    let yes = json!({ "action": "accept", "content": { "confirm": true } });
    let r = complete(
        c.call_once(
            "apply",
            json!({ "plan_id": id }),
            Some(&id),
            Some(responses(&key, yes.clone())),
        )
        .await,
    );
    assert_eq!(r.is_error, Some(true), "{r:?}");
    assert!(env.sys(CACHE).join("blob").exists());
    assert!(history(&env).is_empty());
    assert!(stored(&c, &id));
    // Once asked, the same answer runs it.
    let (asked_key, _, _) = input_required(
        &c.call_once("apply", json!({ "plan_id": id }), None, None)
            .await,
    );
    assert_eq!(asked_key, key);
    let r = complete(
        c.call_once(
            "apply",
            json!({ "plan_id": id }),
            Some(&id),
            Some(responses(&key, yes)),
        )
        .await,
    );
    assert_eq!(data(&r)["applied"], true, "{r:?}");
    c.close().await;
}

#[tokio::test]
async fn plan_that_expires_while_the_user_answers_runs_nothing() {
    let t = inky_with_system();
    let env = t.env();
    let server = test_server(env.clone(), None);
    let shift = server.clock_shift.clone();
    // A slow yes: by the time it arrives the plan has expired.
    let mut asker = Asker::new(Answer::yes());
    asker.while_asked = Some(Arc::new(move || {
        *shift.lock().unwrap() = Some(Duration::from_secs(601));
    }));
    let c = Connected::start_server(server.clone(), asker).await;
    let id = preview(&c, &[]).await;
    let r = apply(&c, &id).await;
    assert_eq!(r.is_error, Some(true), "{r:?}");
    assert_eq!(text(&r), EXPIRED_WHILE_ASKING);
    assert_eq!(c.handler.asked().len(), 1);
    assert!(env.sys(CACHE).join("blob").exists());
    assert!(history(&env).is_empty());
    // The plan stays dropped, even with the clock back.
    *server.clock_shift.lock().unwrap() = None;
    assert!(!stored(&c, &id));
    let r = apply(&c, &id).await;
    assert_eq!(text(&r), PLAN_NOT_FOUND);
    assert_eq!(c.handler.asked().len(), 1);
    c.close().await;
}

#[tokio::test]
async fn failed_confirmation_runs_nothing() {
    let t = inky_with_system();
    let env = t.env();
    let c = connect(env.clone(), Answer::Error).await;
    let id = preview(&c, &[]).await;
    let r = apply(&c, &id).await;
    assert_eq!(r.is_error, Some(true), "{r:?}");
    assert!(text(&r).contains("nothing was run"), "{r:?}");
    assert!(env.sys(CACHE).join("blob").exists());
    assert!(history(&env).is_empty());
    c.close().await;
}

#[tokio::test]
async fn request_and_handshake_must_both_allow_a_form() {
    // The handshake declared no elicitation; this request's `_meta` claims
    // it. Nothing is asked, and the plan stays.
    let t = inky_with_system();
    let env = t.env();
    let c = Connected::start(env.clone(), Asker::incapable()).await;
    let id = preview(&c, &[]).await;
    let mut meta = rmcp::model::RequestMetaObject::new();
    meta.set_client_capabilities(
        rmcp::model::ClientCapabilities::builder()
            .enable_elicitation()
            .build(),
    );
    let r = c
        .call_with_meta("apply", json!({ "plan_id": id }), meta)
        .await;
    assert_eq!(r.is_error, Some(true), "{r:?}");
    assert!(
        text(&r).starts_with("this client can't ask you to confirm"),
        "{r:?}"
    );
    assert!(text(&r).contains("crumbs uninstall inky"), "{r:?}");
    assert!(c.handler.asked().is_empty());
    assert!(stored(&c, &id));
    assert!(env.sys(CACHE).join("blob").exists());
    c.close().await;
}

#[tokio::test]
async fn two_applies_of_one_plan_run_it_once() {
    let t = inky_with_system();
    let env = t.env();
    let c = connect(env.clone(), Answer::yes()).await;
    let id = preview(&c, &[]).await;
    let (a, b) = tokio::join!(apply(&c, &id), apply(&c, &id));
    let ran: Vec<&CallToolResult> = [&a, &b]
        .into_iter()
        .filter(|r| data(r)["applied"] == true)
        .collect();
    assert_eq!(ran.len(), 1, "{a:?} {b:?}");
    let refused: Vec<&CallToolResult> = [&a, &b]
        .into_iter()
        .filter(|r| r.is_error == Some(true))
        .collect();
    assert_eq!(refused.len(), 1, "{a:?} {b:?}");
    assert_eq!(text(refused[0]), PLAN_NOT_FOUND);
    let mut batches: Vec<String> = history(&env).into_iter().map(|e| e.batch).collect();
    batches.dedup();
    assert_eq!(batches.len(), 1, "{batches:?}");
    assert_eq!(c.handler.asked().len(), 1);
    c.close().await;
}

#[tokio::test]
async fn preview_text_carries_no_control_bytes() {
    let t = inky_with_system();
    let env = t.env();
    t.write("/home/user/.cache/inky\x1b[31mred\rback/blob", b"x");
    let c = connect(env.clone(), Answer::Decline).await;
    let r = c
        .call("preview_clean", json!({ "categories": ["cache"] }))
        .await;
    assert_ne!(r.is_error, Some(true), "{r:?}");
    let d = data(&r);
    let shown = d["preview_text"].as_str().unwrap();
    assert!(shown.contains("red"), "{shown:?}");
    let control = |s: &str| s.chars().any(|ch| ch.is_control() && ch != '\n');
    assert!(!control(shown), "{shown:?}");
    let id = d["plan_id"].as_str().unwrap().to_string();
    apply(&c, &id).await;
    let asked = c.handler.asked();
    assert_eq!(asked.len(), 1);
    assert!(!control(&asked[0]), "{:?}", asked[0]);
    c.close().await;
}
