# M4c review follow-ups

M4c is done: `crumbs mcp`, an `rmcp` 3.9.0 stdio MCP server behind the cargo feature `mcp` (on by default), with 11 tools (six read, four preview, `apply`), a single-use plan store, and confirmation through `elicitation/create` (clients that send `initialize`) or `InputRequiredResult` (the 2026-07-28 inline lifecycle). This note lists what the task reviews and rulings accepted or deferred. Each item fails safe today: `apply` never runs without the user's yes, and every M1 to M4b safety check still runs.

## Done in the final wave

1. **`applied` semantics.** `applied` now means at least one action result is `ok` (for a restore, at least one outcome), so a system-only plan with a denied prompt gives `applied: false` and no `report`; a plan whose user-level steps ran before the root steps were denied gives `applied: true` with exit 5. `README.md` and `docs/json-schema.md` say so.

## Protocol and library

2. **MCP logging is deprecated in `rmcp`** (SEP-2577, no replacement yet). Warnings still go out as `notifications/message` at level `warning`, with `allow(deprecated)` and a reason in `log.rs` and `server.rs`. Revisit when the SDK offers another channel.
3. **Which client gets which confirmation path.** The path follows the client's lifecycle, not the version it asks for. A client that sends `initialize` is negotiated to 2025-11-25 even when it asks for 2026-07-28, and gets `elicitation/create`. A client on the 2026-07-28 inline lifecycle (`server/discover`, then per-request `_meta` with `protocolVersion` and `clientCapabilities`) gets `InputRequiredResult`. On that lifecycle `rmcp` rejects a request without `_meta` (-32602) and nothing runs; every other mismatch (capabilities, state, key) fails closed. The final review checked this end to end over stdio, and `multi_round_trip_confirmation_over_stdio` in `crates/crumbs-cli/tests/mcp.rs` now drives the debug binary through it on a fake root: `input_required`, a declined retry that runs nothing, then an accepted retry that trashes the file. No released client has been tried on the inline lifecycle yet.
4. **`rmcp` waits 5 seconds** for in-flight responses after stdin closes. Documented: clients must keep stdin open. Nothing to fix on the crumbs side.
5. **Confirmation timeout.** The server waits 10 minutes for the answer on the elicitation path, so a client with a shorter request timeout reports its own timeout first. The multi round trip path has no server-side wait.

## Behaviour accepted as is

6. **Tool arguments go beyond the spec table**, mirroring the CLI: `user` and `show_low` on `scan`, `orphans` and the previews, `source` as an array, `keep_data` and `leftovers_only`. Same usage rules as the CLI.
7. **`preview_restore`** stores the history entry ids its dry run listed and applies exactly those (never what the selectors match later). Its preview lists destinations only: the core `RestoreOutcome` has no source path. `needs_admin` is always `false`, since system-tree deletes are never restorable.
8. **An empty `select` means the CLI's defaults.** A client cannot select "nothing" other than by not applying the plan.
9. **`expires_at` is `null` when `plan_id` is `null`** (a plan with nothing to do is shown, never stored).
10. **A mismatched multi round trip retry keeps the plan** (a retry for another plan, or with a state or key that does not match). The apply lock covers only the final round there, and the whole confirmation on the elicitation path.
11. **`results[].error` and `manual_commands` are not sanitised** in the structured result, as in the CLI's `--json` (the JSON escaping is the protection). Refusal text and details, the `escalation_output`, the log messages and the `preview_text` (and so the confirmation message) are sanitised.
12. **A running app always refuses** `apply` (`AllowedRunning::None`): no one can be asked whether the app may be closed. The CLI's `--force` has no equivalent.
13. **`apply.rs` is an extra file** beside the spec's layout (`server.rs`, `tools.rs`, `plans.rs`, `confirm.rs` in place of `elicit.rs`).
14. **The debug `Sh` runner writes a line to stderr** in fake roots. Debug builds only.
15. **Plan ids and confirmation keys** come from `/dev/urandom`; when it cannot be read, previews are refused rather than given a weaker id.

## Carried from earlier milestones

The unresolved items of `2026-11-08-m4b-review-followups.md` still stand (process matching, cross-plan dependencies, `/etc` and `/opt` limits). The agent can only act on what those paths find, so they apply to `crumbs mcp` unchanged.

## Not done

16. **No real-client test.** The server was checked with the in-process `rmcp` client, a child-process test over stdio with a fake root, and read-only calls on the real system. It has not been run from a released Claude Code or Claude Desktop, and `apply` was never run on the real system, on purpose.
17. **Out of scope by the spec:** HTTP and SSE transports, MCP resources and prompts, progress notifications, remembered approvals.
18. **AUR, deb and rpm packaging** (milestone 6) should keep the `mcp` feature on, or document `--no-default-features`.
