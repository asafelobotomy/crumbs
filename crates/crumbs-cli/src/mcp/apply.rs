//! `apply`: runs a previewed plan once the user has confirmed it through
//! the client ([`confirm`]), then reports as the CLI would.
//!
//! When a plan stops existing (plan Decision 3): on the first no, cancel
//! or timeout of `elicitation/create`, at the final round of the
//! multi-round-trip path, when it runs, and when it expires. A client that
//! cannot ask the user is refused before the plan is touched.

use crumbs_core::Env;
use rmcp::model::{CallToolResponse, CallToolResult, InputResponses};
use rmcp::service::RequestContext;
use rmcp::{ErrorData as McpError, Peer, RoleServer};
use serde_json::{Value, json};

use super::confirm::{self, Answer, Confirm, PLAN_NOT_FOUND};
use super::plans::{PlanId, PlanPayload, StoredPlan};
use super::server::{PrivilegeFor, Server};
use super::tools::{refusal_result, shell_line};
use crate::cmd::act::{
    self, AllowedRunning, BATCH_STOPPED, EXIT_NO_RIGHTS, NO_RIGHTS, NOT_GRANTED, NOTE_CHECK_MOUNTS,
    TREE_RM_WORDS, Target,
};
use crate::cmd::build::Refusal;
use crate::cmd::context::load_cfg;
use crate::cmd::manual::is_privileged;
use crate::cmd::render::sanitise;
use crate::cmd::restore as restore_cmd;
use crate::output::ErrorEntry;

/// What the tool was called with, beyond its `plan_id`.
pub(super) struct Call {
    pub plan_id: String,
    pub context: RequestContext<RoleServer>,
    /// `requestState` of an MRTR retry: untrusted.
    pub request_state: Option<String>,
    /// `inputResponses` of an MRTR retry: untrusted, except the answer
    /// under the key this server issued.
    pub input_responses: Option<InputResponses>,
}

/// What [`run`] produced: the result data, whether it is a tool error,
/// and the warnings for the log.
type RunOutcome = Result<(Value, bool, Vec<ErrorEntry>), Refusal>;

fn refused(code: u8, msg: impl Into<String>) -> CallToolResponse {
    refusal_result(&Refusal::new(code, msg)).into()
}

/// The refusal of an MRTR retry that does not answer this plan's own
/// confirmation; the plan stays.
const NOT_THIS_CONFIRMATION: &str =
    "that is not the answer to this plan's confirmation; nothing was run";

impl Server {
    pub(super) async fn apply_plan(&self, call: Call) -> Result<CallToolResponse, McpError> {
        self.purge_plans();
        let Some(id) = PlanId::parse(&call.plan_id) else {
            return Ok(refused(2, PLAN_NOT_FOUND));
        };
        let (line, interactive, preview_text, confirm_key) = {
            let plans = self.plans();
            let Some(plan) = plans.peek(&id) else {
                return Ok(refused(2, PLAN_NOT_FOUND));
            };
            (
                shell_line(&plan.cli_command),
                plan.cli_command.last().is_some_and(|a| a == "-i"),
                plan.preview_text.clone(),
                plan.confirm_key.clone(),
            )
        };
        let ctx = &call.context;
        if !confirm::supports_form(ctx.client_capabilities().as_ref()) {
            return Ok(refused(2, confirm::cannot_confirm(&line, interactive)));
        }
        if !confirm::uses_input_required(ctx.protocol_version().as_ref()) {
            return self.apply_elicited(&ctx.peer, &id).await.map(Into::into);
        }
        let Call {
            request_state,
            input_responses,
            ..
        } = call;
        match (request_state, input_responses) {
            // The first round: ask, and keep the plan until the answer.
            (None, None) => match confirm::input_required(id.as_str(), &confirm_key, &preview_text)
            {
                Ok(ask) => Ok(ask.into()),
                Err(e) => Ok(refused(1, format!("cannot ask for confirmation: {e}"))),
            },
            (Some(state), Some(responses)) if state == id.as_str() => {
                let Some(response) = responses.get(&confirm_key) else {
                    return Ok(refused(2, NOT_THIS_CONFIRMATION));
                };
                let answer = confirm::from_input_response(response);
                let _running = self.apply_lock.lock().await;
                self.purge_plans();
                let Some(plan) = self.plans().take(&id) else {
                    return Ok(refused(2, PLAN_NOT_FOUND));
                };
                self.finish(&call.context.peer, plan, answer)
                    .await
                    .map(Into::into)
            }
            _ => Ok(refused(2, NOT_THIS_CONFIRMATION)),
        }
    }

    /// Before protocol 2026-07-28: the plan leaves the store, then the
    /// server asks the user with `elicitation/create`. The lock is held
    /// from the question through the run.
    async fn apply_elicited(
        &self,
        peer: &Peer<RoleServer>,
        id: &PlanId,
    ) -> Result<CallToolResult, McpError> {
        let _running = self.apply_lock.lock().await;
        self.purge_plans();
        let Some(plan) = self.plans().take(id) else {
            return Ok(refusal_result(&Refusal::new(2, PLAN_NOT_FOUND)));
        };
        let asked = peer
            .elicit_with_timeout::<Confirm>(
                confirm::message(&plan.preview_text),
                Some(self.confirm_timeout),
            )
            .await;
        self.finish(peer, plan, confirm::from_elicitation(asked))
            .await
    }

    /// Runs `plan` when `answer` is the user's yes; otherwise says why not.
    async fn finish(
        &self,
        peer: &Peer<RoleServer>,
        plan: StoredPlan,
        answer: Answer,
    ) -> Result<CallToolResult, McpError> {
        if let Some(reason) = answer.reason() {
            return Ok(CallToolResult::structured(
                json!({ "applied": false, "reason": reason }),
            ));
        }
        if let Answer::Failed(e) = answer {
            return Ok(refusal_result(&Refusal::new(
                1,
                format!("the confirmation failed: {e}; nothing was run"),
            )));
        }
        let env = self.env.clone();
        let privilege = self.privilege.clone();
        let outcome = tokio::task::spawn_blocking(move || run(&env, &privilege, plan.payload))
            .await
            .map_err(|e| McpError::internal_error(format!("crumbs stopped: {e}"), None))?;
        Ok(match outcome {
            Ok((data, is_error, warnings)) => {
                self.log.warn(peer, &warnings).await;
                let mut result = CallToolResult::structured(data);
                if is_error {
                    result.is_error = Some(true);
                }
                result
            }
            Err(refusal) => {
                self.log.flush(peer).await;
                refusal_result(&refusal)
            }
        })
    }
}

/// Runs a confirmed payload on the blocking pool.
fn run(env: &Env, privilege: &PrivilegeFor, payload: PlanPayload) -> RunOutcome {
    match payload {
        PlanPayload::Plans(targets) => run_plans(env, privilege, &targets),
        PlanPayload::Restore { entry_ids } => run_restore(env, &entry_ids),
    }
}

/// The plans through the CLI's executor path: every safety re-check, one
/// privileged batch, the history.
fn run_plans(env: &Env, privilege: &PrivilegeFor, targets: &[Target]) -> RunOutcome {
    let (cfg, warnings) = load_cfg(env).map_err(|e| Refusal::new(1, e.to_string()))?;
    let needed = targets
        .iter()
        .flat_map(|t| &t.plan.actions)
        .any(is_privileged);
    // No one to ask whether a running app may go on: it may not.
    let outcome = act::execute_confirmed(
        env,
        &cfg,
        targets,
        privilege(env, needed),
        AllowedRunning::None,
    );
    if outcome.reports.is_empty()
        && let Some(running) = outcome.running.first()
    {
        return Err(Refusal::new(
            1,
            format!("{running}; close it, then preview again"),
        ));
    }
    let reports = &outcome.reports;
    if outcome.exit_code == EXIT_NO_RIGHTS {
        let not_granted = reports.iter().flatten().any(|r| r.escalation_failed);
        let mut data = json!({
            "applied": false,
            "exit_code": EXIT_NO_RIGHTS,
            "error": if not_granted { NOT_GRANTED } else { NO_RIGHTS },
            "manual_commands": outcome.manual_commands,
        });
        if outcome
            .manual_commands
            .iter()
            .any(|m| m.contains(TREE_RM_WORDS))
        {
            data["note"] = json!(NOTE_CHECK_MOUNTS);
        }
        if !reports.is_empty() {
            // What did not need the rights ran.
            data["report"] = json!(reports);
        }
        let output: Vec<u8> = reports
            .iter()
            .flatten()
            .map(|r| r.escalation_output.as_slice())
            .find(|o| !o.is_empty())
            .unwrap_or_default()
            .to_vec();
        if !output.is_empty() {
            data["escalation_output"] = json!(sanitise(&String::from_utf8_lossy(&output)));
        }
        return Ok((data, false, warnings));
    }
    let mut data = json!({
        "applied": true,
        "report": reports,
        "exit_code": outcome.exit_code,
    });
    if reports.iter().flatten().any(|r| r.batch_stopped) {
        data["error"] = json!(BATCH_STOPPED);
    }
    Ok((data, outcome.exit_code == 3, warnings))
}

/// Restores exactly the history entries the preview listed.
fn run_restore(env: &Env, entry_ids: &[String]) -> RunOutcome {
    let (outcomes, warnings) =
        restore_cmd::outcomes(env, entry_ids, false).map_err(|e| Refusal::new(1, e.to_string()))?;
    let all_ok = outcomes.iter().all(|o| o.ok);
    let exit_code = if all_ok { 0 } else { 3 };
    let data = json!({ "applied": true, "outcomes": outcomes, "exit_code": exit_code });
    Ok((data, !all_ok, warnings))
}
