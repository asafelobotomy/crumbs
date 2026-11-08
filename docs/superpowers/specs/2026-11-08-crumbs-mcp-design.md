# crumBS MCP server — design

**Status:** approved in conversation on 2026-11-08; awaiting written-spec review.
**Builds on:** `docs/superpowers/specs/2026-11-08-crumbs-design.md` (the main spec) and milestone 4b. 4b's Task 8 splits `cmd/act.rs` so that planning and rendering can run without a terminal.

## 1. Purpose

AI agents (Claude Code, Claude Desktop and other MCP clients) can use crumbs to inspect a machine and prepare clean-ups. Nothing changes until **the user** confirms the exact plan.

**What the user decided:**
- Agents can inspect, and can preview any plan.
- Acting needs a separate `apply` step.
- The confirmation is asked by the MCP client through **elicitation**, so it reaches the user directly and is outside the agent's control.
- Applied plans may include root actions. These go through the normal run0/pkexec polkit prompt.
- The server ships as a `crumbs mcp` subcommand over stdio. It is built on the official Rust MCP SDK (`rmcp`), behind a cargo feature.

**Success criteria:**
- An agent can answer "what is using space?", "what would uninstalling X remove?" and "what can be cleaned?" with the same data the CLI shows.
- No tool call changes the system unless the user said yes to that exact plan in an elicitation prompt.
- Every M1–M4b safety property holds unchanged. This covers fingerprints, re-verification right before acting, protected paths, the single privileged batch, and history.

**Non-goals:**
- An HTTP or SSE transport.
- MCP resources and prompts.
- Long-running jobs and progress notifications.
- Remembered approvals.
- Letting agents add paths crumbs did not find.

## 2. Tools

| Tool | Arguments | Result | Changes anything? |
|---|---|---|---|
| `doctor` | — | `doctor --json` data | no |
| `apps_list` | `source?`, `all?` | `apps list --json` data | no |
| `app_show` | `app` | app details plus leftovers (confidence, size, reason, scope) | no |
| `scan` | `system?`, `categories?` | `scan --json` data | no |
| `orphans` | `system?` | `orphans --json` data | no |
| `history` | `limit?` (default 50) | recent history entries | no |
| `preview_uninstall` | `app`, `leftovers_only?`, `keep_data?`, `select?` | plan + `plan_id` | no |
| `preview_clean` | `categories?`, `system?`, `select?` | plan + `plan_id` | no |
| `preview_orphans` | `paths`, `system?` | plan + `plan_id` | no |
| `preview_restore` | `entry` | plan + `plan_id` | no |
| `apply` | `plan_id` | the execution report | **yes, after the user confirms** |

**Results.** Result data has exactly the shapes documented in `docs/json-schema.md`. A preview result also carries:
- `plan_id`;
- `expires_at`, an RFC 3339 timestamp;
- `needs_admin`, a bool;
- `preview_text`, the human preview exactly as the CLI prints it, without colour.

**Selection (`select`).** `select` takes paths, category ids or item ids that crumbs itself returned for that same request.
- A value crumbs did not find is refused with a tool error naming it.
- Without `select`, the CLI's defaults apply.
- Items that are never pre-selected are included only when named in `select`. These are system leftovers, opt-in categories and Low findings.

**Arguments.** They mirror the CLI flags, including the same usage rules. For example, `user` and `system` together is an error.

## 3. Architecture

**Code layout:**
- `crates/crumbs-cli/src/mcp/` holds `server.rs`, `tools.rs`, `plans.rs` and `elicit.rs`.
- It sits behind the cargo feature `mcp`, which is on by default. `--no-default-features` builds the CLI without `rmcp`.
- The clap subcommand `mcp` exists only with the feature.

**Sharing code with the CLI:**
- Each tool calls the same core functions the CLI commands use: discovery, leftovers, scanners, planning, `plan_unmanaged`, `execute_many`, history and restore.
- It also calls the non-prompting plan-building and rendering functions that 4b Task 8 extracts from `cmd/act`.
- No tool shells out to `crumbs`.

**Plan store** (`plans.rs`):
- An in-memory `HashMap<PlanId, StoredPlan>`.
- `PlanId` is 128 random bits shown as hex, read from `/dev/urandom` through `std::fs` (no new crate).
- `StoredPlan` holds the plans as built (with their scan-time fingerprints), the rendered `preview_text`, `needs_admin`, the equivalent CLI command, and `expires_at` (creation + 10 minutes).
- Expired entries are purged on every call.
- A restart drops every plan.

**Apply flow:**
1. Remove the plan from the store. If it is unknown or expired, return a tool error `plan not found or expired; preview again`. Removing first makes a plan single-use, even when apply then fails or is declined.
2. If the client did not advertise the elicitation capability, return a tool error. The error names the equivalent `crumbs …` command for the user to run in a terminal, and nothing runs.
3. Send `elicitation/create`:
   - the message is `preview_text` followed by `Apply this plan?`;
   - the schema is `{ confirm: boolean }`, required.
4. If the user declines or cancels, or `confirm` is false, return a normal result `{ applied: false, reason: "declined" }`.
5. Otherwise run the plans through `execute_many`, using the same privilege detection as the CLI, and return the report.
   - Every safety re-check runs exactly as in the CLI, and history is written.

## 4. Running inside an MCP server

**Privilege and stdio.** stdin and stdout carry the protocol, so:
- The escalation process gets stdin from `/dev/null`, its stdout captured as today, and stderr sent to the server's log (MCP logging notifications).
- `sudo` is never used here, because it needs a TTY.
- run0 and pkexec prompt through the user's polkit agent. The same is true of the MCP `doctor` tool's escalation line.
- With no usable escalation, `apply` returns the M4 "no way to get administrator rights" result, with `manual_commands`. That is the CLI's exit-5 path, and it happens after the user's elicitation yes.

**Output and signals:**
- **stdout:** nothing but protocol frames.
- **Warnings:** these are config and rule warnings, history mode-tightening and similar. They become MCP log messages at level `warning`.
- **Signals:** the privileged batch holds SIGINT and SIGQUIT exactly as the CLI does, and only for the batch.

**Concurrency.** Tool calls are handled one at a time. A second `apply` while one runs waits its turn, so two privileged batches never overlap.

## 5. Errors

**Tool errors:**
- **Usage and domain errors:** an unknown app, a refused unmanaged removal, Busy, conflicting flags or an invalid `select`. These are tool errors (`isError: true`) whose text is the CLI's message, plus `exit_code`, the CLI exit code the same situation would produce.
- **Partial failure:** CLI exit 3. The result holds the full report, with `isError: true`.

**Normal results:**
- **Declined:** a normal result, not an error.
- **Escalation unavailable or denied:** a normal result with `manual_commands` and `exit_code: 5`.

## 6. Testing

**Setup:**
- An in-process `rmcp` client talks to the server over `tokio::io::duplex`.
- Fixtures use the existing fake roots and `FakePrivilegedRunner`.
- A test elicitation handler answers yes, no or cancel, or advertises no elicitation support.

**Tests that must exist:**
- `apply_without_confirmation_changes_nothing` covers each of: no, cancel, and unsupported.
- `apply_confirmed_runs_plan_and_writes_history`.
- `plan_id_is_single_use`.
- `expired_plan_is_refused`, with the clock shifted via the existing test hook.
- `select_rejects_unknown_paths`.
- `system_leftovers_need_explicit_select`.
- `stdout_carries_only_protocol`: run `crumbs mcp` as a child process with a fake root, send `initialize` and a `scan` call, and require that every stdout line parses as JSON-RPC.
- `privileged_plan_uses_one_batch_and_dev_null_stdin`.

**Global rules:** these come from the M4 and 4b global constraints, and still apply.
- No release-profile tests.
- No real escalation.
- No `apply` against the real system.
- Read-only real-system checks only: the read tools and the previews.

## 7. Milestone

This is a new milestone, **M4c: MCP server**, after 4b and before M5.
- It depends on 4b's `act` split.
- The `rmcp` version and its elicitation API are pinned in the plan, after a check against the crate docs. Elicitation support in `rmcp` is a prerequisite; if the pinned version lacks it, the plan must say so before any task starts.
- Docs: a README section "Using crumbs from an AI agent", with client config snippets for Claude Code and Claude Desktop, and `docs/json-schema.md` gains the preview and apply envelopes.
