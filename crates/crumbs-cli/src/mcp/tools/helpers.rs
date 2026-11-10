//! Shared tool plumbing: refusals, previews and the read/plan helpers on Server.

use super::*;

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

/// The error for a blocking task that never finished (it panicked or was
/// cancelled), its text sanitised.
pub(in crate::mcp) fn crumbs_stopped(e: &dyn std::fmt::Display) -> McpError {
    McpError::internal_error(sanitise(&format!("crumbs stopped: {e}")), None)
}

/// Flags for a preview: it only plans.
pub(super) fn dry_run_flags() -> ActFlags {
    ActFlags {
        dry_run: true,
        json: true,
        ..ActFlags::default()
    }
}

/// Each `name` as `T`, or the usage error clap would give.
pub(super) fn parse_all<T: std::str::FromStr<Err = String>>(
    names: &[String],
) -> Result<Vec<T>, Refusal> {
    names
        .iter()
        .map(|n| n.parse().map_err(|e: String| Refusal::new(2, e)))
        .collect()
}

/// A preview before it is stored: what the result holds, and what
/// `apply` would run.
pub(super) struct Preview {
    pub(super) payload: PlanPayload,
    /// `dry_run_data`, an object.
    pub(super) data: Value,
    pub(super) preview_text: String,
    pub(super) needs_admin: bool,
    pub(super) cli_command: Vec<String>,
    pub(super) errors: Vec<ErrorEntry>,
    /// A plan with nothing to do is shown, never stored.
    pub(super) has_actions: bool,
}

impl Preview {
    pub(in crate::mcp) fn of_plans(env: &Env, built: Built) -> Preview {
        let Built {
            targets,
            errors,
            after_preview,
            cli_command,
            ..
        } = built;
        let actions = || targets.iter().flat_map(|t| &t.plan.actions);
        Preview {
            data: act::dry_run_data(&targets),
            preview_text: act::preview_text(env, &targets, &after_preview, &errors),
            needs_admin: actions().any(is_privileged),
            has_actions: actions().next().is_some(),
            payload: PlanPayload::Plans(targets),
            cli_command,
            errors,
        }
    }
}

/// `argv` as one line for a shell: arguments with anything but plain
/// characters are single-quoted.
pub(in crate::mcp) fn shell_line(argv: &[String]) -> String {
    use std::ffi::OsStr;
    let plain = |a: &str| {
        !a.is_empty()
            && a.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_@%+=:,./-".contains(&b))
    };
    let words: Vec<String> = argv
        .iter()
        .map(|a| {
            if plain(a) {
                a.clone()
            } else {
                let quoted = crumbs_core::privilege::batch::sh_quote(OsStr::new(a));
                String::from_utf8_lossy(&quoted).into_owned()
            }
        })
        .collect();
    words.join(" ")
}

/// `select` as the builders take it: none means the CLI's defaults.
pub(super) fn selection<T: From<String>>(select: Vec<String>) -> Option<Vec<T>> {
    (!select.is_empty()).then(|| select.into_iter().map(T::from).collect())
}

impl Server {
    pub(in crate::mcp) fn plans(&self) -> MutexGuard<'_, crate::mcp::plans::PlanStore> {
        self.plans
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Drops the plans that have expired; every tool call starts here.
    pub(in crate::mcp) fn purge_plans(&self) {
        self.plans().purge_expired(self.now());
    }

    /// Runs `f` off the async thread, stores what it previews and returns
    /// the plan, its `plan_id` and the human preview.
    pub(super) async fn preview<F>(
        &self,
        peer: Peer<RoleServer>,
        f: F,
    ) -> Result<CallToolResult, McpError>
    where
        F: FnOnce(&Env) -> Result<Preview, Refusal> + Send + 'static,
    {
        self.purge_plans();
        let env = self.env.clone();
        let outcome = tokio::task::spawn_blocking(move || f(&env))
            .await
            .map_err(|e| crumbs_stopped(&e))?;
        let preview = match outcome {
            Ok(p) => p,
            Err(refusal) => {
                self.log.flush(&peer).await;
                return Ok(refusal_result(&refusal));
            }
        };
        self.log.warn(&peer, &preview.errors).await;
        let Preview {
            payload,
            mut data,
            preview_text,
            needs_admin,
            cli_command,
            has_actions,
            ..
        } = preview;
        let line = shell_line(&cli_command);
        let mut plan_id = Value::Null;
        let mut expires = Value::Null;
        if has_actions {
            let expires_at = self.now() + PLAN_TTL;
            let Some(expires_text) = crumbs_core::history::rfc3339(expires_at) else {
                return Ok(refusal_result(&Refusal::new(
                    1,
                    "the plan's expiry time is out of range",
                )));
            };
            let stored = StoredPlan {
                payload,
                preview_text: preview_text.clone(),
                cli_command,
                expires_at,
                confirm_key: String::new(),
                asked: false,
            };
            match self.plans().insert(stored) {
                Ok(id) => {
                    plan_id = json!(id.as_str());
                    expires = json!(expires_text);
                }
                Err(e) => return Ok(refusal_result(&Refusal::new(1, e.to_string()))),
            }
        }
        data["cli_command"] = json!(line);
        data["plan_id"] = plan_id;
        data["expires_at"] = expires;
        data["needs_admin"] = json!(needs_admin);
        data["preview_text"] = json!(preview_text);
        Ok(CallToolResult::structured(data))
    }

    /// Runs `f` off the async thread, then sends its warnings and returns
    /// its data.
    pub(super) async fn read<F>(
        &self,
        peer: Peer<RoleServer>,
        f: F,
    ) -> Result<CallToolResult, McpError>
    where
        F: FnOnce(&Env) -> ReadOutcome + Send + 'static,
    {
        self.purge_plans();
        let env = self.env.clone();
        let outcome = tokio::task::spawn_blocking(move || f(&env))
            .await
            .map_err(|e| crumbs_stopped(&e))?;
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
