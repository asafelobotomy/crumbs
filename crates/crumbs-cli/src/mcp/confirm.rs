//! The user's yes to a plan, asked through the MCP client: the one gate
//! before `apply` changes anything.
//!
//! Only two answers can confirm, and both are protocol fields the client
//! fills from the user's answer, never tool arguments:
//! - before protocol 2026-07-28, the result of the server's own
//!   `elicitation/create` request ([`from_elicitation`]);
//! - from 2026-07-28 on, the `inputResponses` entry under the key this
//!   server issued for the plan ([`from_input_response`]).
//!
//! Either must say `accept` with `confirm: true`. Anything else is no.

use std::path::Path;
use std::time::Duration;

use rmcp::model::{
    ClientCapabilities, ElicitRequest, ElicitRequestParams, ElicitResult, ElicitationAction,
    ElicitationSchema, InputRequest, InputRequests, InputRequiredResult, ProtocolVersion,
};
use rmcp::service::{ElicitationError, ServiceError};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;

/// What the user is asked: the preview, then this.
pub(crate) const CONFIRM_SUFFIX: &str = "\n\nApply this plan?";

/// How long the server waits for the user's answer to `elicitation/create`.
pub(crate) const CONFIRM_TIMEOUT: Duration = Duration::from_secs(600);

/// The refusal for an unknown, used or expired plan id.
pub(crate) const PLAN_NOT_FOUND: &str = "plan not found or expired; preview again";

/// The answer the client's form collects.
#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct Confirm {
    /// True to apply the plan as shown.
    pub confirm: bool,
}

rmcp::elicit_safe!(Confirm);

/// The message of the confirmation: the preview the CLI would show, then
/// the question.
pub(crate) fn message(preview_text: &str) -> String {
    format!("{preview_text}{CONFIRM_SUFFIX}")
}

/// How a confirmation round ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Answer {
    /// `accept` with `confirm: true`: the only yes.
    Confirmed,
    Declined,
    Cancelled,
    TimedOut,
    /// The request itself failed (the client went away, an error reply).
    Failed(String),
}

impl Answer {
    /// The `reason` of `{ applied: false, reason }`, for a normal "no".
    pub(crate) fn reason(&self) -> Option<&'static str> {
        match self {
            Answer::Declined => Some("declined"),
            Answer::Cancelled => Some("cancelled"),
            Answer::TimedOut => Some("timed out"),
            Answer::Confirmed | Answer::Failed(_) => None,
        }
    }
}

/// The answer to the server's `elicitation/create`. An `accept` without
/// content, with content that is not `{ confirm: bool }`, or with
/// `confirm: false` is a decline.
pub(crate) fn from_elicitation(result: Result<Option<Confirm>, ElicitationError>) -> Answer {
    match result {
        Ok(Some(Confirm { confirm: true })) => Answer::Confirmed,
        Ok(_) => Answer::Declined,
        Err(ElicitationError::UserDeclined)
        | Err(ElicitationError::NoContent)
        | Err(ElicitationError::ParseError { .. }) => Answer::Declined,
        Err(ElicitationError::UserCancelled) => Answer::Cancelled,
        Err(ElicitationError::Service(ServiceError::Timeout { .. })) => Answer::TimedOut,
        Err(e) => Answer::Failed(e.to_string()),
    }
}

/// The answer in one `inputResponses` entry (an `ElicitResult` as JSON),
/// on the same rules as [`from_elicitation`]. Anything that is not an
/// `ElicitResult` is a decline.
pub(crate) fn from_input_response(response: &Value) -> Answer {
    let Ok(result) = serde_json::from_value::<ElicitResult>(response.clone()) else {
        return Answer::Declined;
    };
    match result.action {
        ElicitationAction::Accept => {
            let confirm = result
                .content
                .and_then(|c| serde_json::from_value::<Confirm>(c).ok());
            match confirm {
                Some(Confirm { confirm: true }) => Answer::Confirmed,
                _ => Answer::Declined,
            }
        }
        ElicitationAction::Cancel => Answer::Cancelled,
        // Decline, and anything a later protocol adds.
        _ => Answer::Declined,
    }
}

/// Whether `version` asks for confirmation by `InputRequiredResult` rather
/// than by a server-to-client request.
pub(crate) fn uses_input_required(version: Option<&ProtocolVersion>) -> bool {
    version.is_some_and(|v| *v >= ProtocolVersion::V_2026_07_28)
}

/// Whether the client can show a form: it declared elicitation, with form
/// mode or (as before modes existed) no mode at all.
pub(crate) fn supports_form(caps: Option<&ClientCapabilities>) -> bool {
    caps.and_then(|c| c.elicitation.as_ref())
        .is_some_and(|e| e.form.is_some() || e.url.is_none())
}

/// The first round of the multi-round-trip confirmation: one form
/// elicitation under `key`, and `plan_id` as the request state.
pub(crate) fn input_required(
    plan_id: &str,
    key: &str,
    preview_text: &str,
) -> Result<InputRequiredResult, serde_json::Error> {
    let request = ElicitRequest::new(ElicitRequestParams::FormElicitationParams {
        meta: None,
        message: message(preview_text),
        requested_schema: ElicitationSchema::from_type::<Confirm>()?,
    });
    let mut requests = InputRequests::new();
    requests.insert(key.to_string(), InputRequest::Elicitation(request));
    Ok(InputRequiredResult::new(
        Some(requests),
        Some(plan_id.to_string()),
    ))
}

/// The refusal for a client that cannot ask the user: the command to run
/// in a terminal instead.
pub(crate) fn cannot_confirm(command_line: &str, interactive: bool) -> String {
    let mut msg = format!(
        "this client can't ask you to confirm; run this in a terminal instead: {command_line}"
    );
    if interactive {
        msg.push_str(" (pick the same items with -i)");
    }
    msg
}

/// How long to wait for the user: [`CONFIRM_TIMEOUT`], or in a debug build
/// under a fake root, `test_ms` milliseconds (`CRUMBS_TEST_CONFIRM_TIMEOUT_MS`)
/// when set.
pub(crate) fn timeout(root: &Path, test_ms: Option<&str>) -> Duration {
    #[cfg(debug_assertions)]
    if root != Path::new("/")
        && let Some(ms) = test_ms.and_then(|v| v.parse().ok())
    {
        return Duration::from_millis(ms);
    }
    let _ = (root, test_ms);
    CONFIRM_TIMEOUT
}

#[cfg(test)]
mod tests {
    use rmcp::model::ElicitationCapability;
    use serde_json::json;

    use super::*;

    fn parse_error() -> ElicitationError {
        let error = serde_json::from_value::<Confirm>(json!({})).unwrap_err();
        ElicitationError::ParseError {
            error,
            data: json!({}),
        }
    }

    #[test]
    fn only_accept_with_confirm_true_confirms_an_elicitation() {
        assert_eq!(
            from_elicitation(Ok(Some(Confirm { confirm: true }))),
            Answer::Confirmed
        );
        for no in [
            Ok(Some(Confirm { confirm: false })),
            Ok(None),
            Err(ElicitationError::UserDeclined),
            Err(ElicitationError::NoContent),
            Err(parse_error()),
        ] {
            assert_eq!(from_elicitation(no), Answer::Declined);
        }
        assert_eq!(
            from_elicitation(Err(ElicitationError::UserCancelled)),
            Answer::Cancelled
        );
        let timeout = ServiceError::Timeout {
            timeout: Duration::from_millis(1),
        };
        assert_eq!(
            from_elicitation(Err(ElicitationError::Service(timeout))),
            Answer::TimedOut
        );
        assert!(matches!(
            from_elicitation(Err(ElicitationError::Service(
                ServiceError::TransportClosed
            ))),
            Answer::Failed(_)
        ));
    }

    #[test]
    fn only_accept_with_confirm_true_confirms_an_input_response() {
        assert_eq!(
            from_input_response(&json!({ "action": "accept", "content": { "confirm": true } })),
            Answer::Confirmed
        );
        for no in [
            json!({ "action": "accept", "content": { "confirm": false } }),
            json!({ "action": "accept" }),
            json!({ "action": "accept", "content": {} }),
            json!({ "action": "accept", "content": { "confirm": "true" } }),
            json!({ "action": "accept", "content": { "confirm": 1 } }),
            json!({ "action": "decline", "content": { "confirm": true } }),
            json!({ "confirm": true }),
            json!(true),
            json!(null),
        ] {
            assert_eq!(from_input_response(&no), Answer::Declined, "{no}");
        }
        assert_eq!(
            from_input_response(&json!({ "action": "cancel", "content": { "confirm": true } })),
            Answer::Cancelled
        );
    }

    #[test]
    fn input_required_asks_one_form_under_the_key_with_the_plan_as_state() {
        let r = input_required("ab12", "confirm-xyz", "PLAN").unwrap();
        let v = serde_json::to_value(&r).unwrap();
        assert_eq!(v["resultType"], "input_required");
        assert_eq!(v["requestState"], "ab12");
        let requests = v["inputRequests"].as_object().unwrap();
        assert_eq!(requests.len(), 1);
        let req = &requests["confirm-xyz"];
        assert_eq!(req["method"], "elicitation/create");
        assert_eq!(req["params"]["message"], "PLAN\n\nApply this plan?");
        let schema = &req["params"]["requestedSchema"];
        assert_eq!(schema["properties"]["confirm"]["type"], "boolean");
        assert_eq!(schema["required"], json!(["confirm"]));
    }

    #[test]
    fn protocol_version_picks_the_path() {
        assert!(!uses_input_required(None));
        assert!(!uses_input_required(Some(&ProtocolVersion::V_2025_11_25)));
        assert!(uses_input_required(Some(&ProtocolVersion::V_2026_07_28)));
    }

    #[test]
    fn form_support_follows_the_capability() {
        assert!(!supports_form(None));
        assert!(!supports_form(Some(&ClientCapabilities::default())));
        let mut caps = ClientCapabilities::default();
        caps.elicitation = Some(ElicitationCapability::new());
        assert!(supports_form(Some(&caps)));
        let url_only: ElicitationCapability = serde_json::from_value(json!({ "url": {} })).unwrap();
        caps.elicitation = Some(url_only);
        assert!(!supports_form(Some(&caps)));
        let form: ElicitationCapability = serde_json::from_value(json!({ "form": {} })).unwrap();
        caps.elicitation = Some(form);
        assert!(supports_form(Some(&caps)));
    }

    #[test]
    fn test_timeout_only_under_a_fake_root() {
        let fake = Path::new("/tmp/fixture");
        assert_eq!(timeout(fake, None), CONFIRM_TIMEOUT);
        assert_eq!(timeout(fake, Some("200")), Duration::from_millis(200));
        assert_eq!(timeout(fake, Some("x")), CONFIRM_TIMEOUT);
        assert_eq!(timeout(Path::new("/"), Some("200")), CONFIRM_TIMEOUT);
    }

    #[test]
    fn cannot_confirm_names_the_command() {
        assert_eq!(
            cannot_confirm("crumbs uninstall inky", false),
            "this client can't ask you to confirm; run this in a terminal instead: \
             crumbs uninstall inky"
        );
        assert!(cannot_confirm("crumbs uninstall inky -i", true).ends_with("with -i)"));
    }
}
