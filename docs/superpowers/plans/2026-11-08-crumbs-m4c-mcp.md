# crumBS Milestone 4c (MCP Server) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `crumbs mcp` serves the crumbs features to AI agents over stdio.
- Agents can inspect anything and preview any plan.
- A plan runs only after the user says yes, through the MCP client's own confirmation (elicitation).

**Architecture:**
- **Command split:** CLI commands are split into pure "build" functions, which return targets, data and refusals, and thin printers. Both the CLI and the MCP tools call the build functions.
- **Rendering:** the human preview is rendered into a `String`, so the MCP server can show the user exactly what the CLI would print.
- **Server:** a feature-gated `mcp` module in `crumbs-cli` runs an `rmcp` 3.9.0 stdio server. It holds an in-memory, single-use plan store and runs `apply` through the same executor path as the CLI.
- **Confirmation:** a plan runs only after a confirmation that reaches the user through the client. Two paths:
  - server-to-client `elicitation/create` for protocol versions before 2026-07-28;
  - an `InputRequiredResult` (multi-round-trip) for 2026-07-28 and later.

**Tech Stack:** the M4b stack plus, behind the cargo feature `mcp`, `rmcp = "=3.9.0"` with features `server`, `macros`, `transport-io`, `elicitation` and `schemars`, plus `tokio` (`rt`, `macros`, `io-std`, `sync`) and `schemars`.

**Spec:** `docs/superpowers/specs/2026-11-08-crumbs-mcp-design.md` (approved 2026-11-08). The main spec is `docs/superpowers/specs/2026-11-08-crumbs-design.md`.

## Decisions this plan makes (the spec leaves them open)

1. **Two confirmation paths.** `rmcp` 3.9.0 refuses server-to-client requests on protocol `2026-07-28` and later, and offers multi-round-trip `InputRequiredResult` instead (`rmcp/src/model/mrtr.rs`). `apply` therefore checks `peer.protocol_version()`:
   - **Before 2026-07-28:** call `peer.elicit::<Confirm>(message)`, where `struct Confirm { confirm: bool }` is marked with `rmcp::elicit_safe!`.
   - **2026-07-28 or later:** on the first call, return an `InputRequiredResult` whose single input request is an `ElicitRequest` with the same message and schema. Its `requestState` is the plan id. The client retries `apply` with `input_responses`.
     - Accept only a response to that request whose action is `accept` and whose `confirm` is `true`.
     - The plan stays in the store between the two rounds. The retry must name the same plan id in its arguments and in `requestState`, or it is refused.
   - **Neither path available:** when `supported_elicitation_modes()` lacks `Form`, `apply` refuses before removing the plan, with `this client can't ask you to confirm; run this in a terminal instead: <cli command>`.
2. **Where elicitation answers come from.** Only `elicitation/create` responses and `input_responses` count as confirmation. These are protocol fields the client fills from the user's answer. No tool argument can confirm a plan: `apply` takes only `plan_id`.
3. **When a plan stops existing.** A plan is removed from the store:
   - when its first confirmation round is declined, cancelled or times out (elicitation timeout 10 minutes);
   - when it runs;
   - when it expires.

   A retry after removal gets `plan not found or expired; preview again`. Under the multi-round-trip path the plan is removed only at the final round. A plan whose MRTR round is pending still expires at its original `expires_at`.
4. **The CLI command shown to the user.** Each stored plan carries the exact CLI command that would produce the same plan, for example `crumbs uninstall inky --leftovers-only`. Arguments are quoted with `privilege::batch::sh_quote` rendering. Selections become repeated `--select` flags where the CLI has them. For `uninstall` selections, which have no CLI flag, the command is the plain one with `-i` appended, and the message says `pick the same items with -i`.
5. **Privilege under MCP.**
   - The escalation process's stdin is `/dev/null` and its stderr is captured into the server log. `sudo` is excluded, because it requires a TTY.
   - The core `SystemPrivilegedRunner` gains a constructor option for this. The CLI's terminal behaviour is unchanged.
6. **Server threading.** Tool handlers run blocking crumbs code with `tokio::task::spawn_blocking`. A single `tokio::sync::Mutex<()>` serialises every `apply`, so two privileged batches never overlap. Read tools are not serialised.
7. **Logging.** Warnings that the CLI prints to stderr become MCP `notifications/message` at level `warning`. To do that, every core and CLI function the tools call returns its warnings instead of printing them. Task 1 audits this; anything left over is routed through one `Warn` sink.
8. **Out of scope for M4c:** HTTP and SSE transports, MCP resources and prompts, progress notifications, and remembered approvals.

## Global Constraints

- **Inherited:** all M1–M4b global constraints still apply. See the M4b plan, "Global Constraints".
- **New dependencies:** only `rmcp = "=3.9.0"`, `tokio` and `schemars`, all optional and enabled only by the cargo feature `mcp`. The feature is on by default. `cargo build --no-default-features -p crumbs-cli` must build without them.
- **MSRV:** the workspace has `rust-version = "1.99"` and `rmcp` requires 1.88, so no MSRV change.
- **stdout:** under `crumbs mcp`, nothing but MCP protocol frames ever reaches stdout. That rules out `println!`, anstream stdout and preview printing in the server process.
- **Never trigger a real privilege escalation in tests or verification:**
  - Tests use fake roots and `FakePrivilegedRunner`, or the debug `CRUMBS_TEST_PRIV_RUNNER`.
  - Never run `cargo test --release`.
  - Never call `apply` against the real system.
  - Real-system checks are limited to the read tools and the preview tools.
- **The one gate:** `apply` never runs anything without a confirmation from Decision 1. Every safety re-check of M1–M4b still runs in `execute_many`.

## Review Focus

1. **A confirmation that was never given.** For example: a client that answers `accept` with `confirm: false`, an `accept` with no content, or an MRTR retry whose `input_responses` answers a different request id. Each must run nothing. Pinned by `apply_rejects_accept_without_confirm_true` (Task 5).
2. **A plan applied twice, or applied after the world changed.** A second `apply` with the same id must be refused. A file swapped between preview and apply must be refused by the executor's fingerprint check. Pinned by `plan_id_is_single_use` and `apply_refuses_swapped_target` (Task 5).
3. **An agent naming a path crumbs never found.** For example `select: ["/etc/passwd"]` or `["~/.ssh"]`. It must be refused before a plan exists. Pinned by `select_rejects_unknown_paths` (Task 4).
4. **Anything else on stdout.** A warning, a colour code or a stray print on stdout corrupts the protocol and kills the session. Pinned by `stdout_carries_only_protocol` (Task 3), which drives a real `crumbs mcp` child with config and rule warnings present.
5. **A slow or absent user.** The user does not answer the confirmation. The server must not hold the apply mutex forever, and the plan must not be applied later without a fresh yes. Pinned by `unanswered_confirmation_times_out_and_drops_plan` (Task 5), which uses a short test timeout.

---

### Task 1: Command builders and rendering to strings

**Files:**
- Modify:
  - `crates/crumbs-cli/src/cmd/{uninstall,clean,orphans,restore,scan,apps,history,doctor}.rs`
  - `cmd/render.rs`
  - `cmd/act/mod.rs`
- Test: `crates/crumbs-cli/tests/cli.rs`. Every existing test and every insta snapshot must pass unchanged.

**Interfaces:**
- Produces, in a new `crates/crumbs-cli/src/cmd/build.rs`:
  - `pub struct Built { pub targets: Vec<Target<'static>>, pub errors: Vec<ErrorEntry>, pub after_preview: Vec<String>, pub selectable: Vec<Finding>, pub cli_command: Vec<String> }`. Use owned headings, and change `Target`'s lifetime if needed.
  - `pub struct Refusal { pub exit_code: u8, pub message: String, pub details: Vec<String> }`
  - `pub fn uninstall(env: &Env, args: &UninstallArgs, select: Option<&[PathBuf]>) -> Result<Built, Refusal>`
  - `pub fn clean(env: &Env, args: &CleanArgs, select: Option<&[String]>) -> Result<Built, Refusal>`
  - `pub fn orphans(env: &Env, args: &OrphansArgs, select: &[PathBuf]) -> Result<Built, Refusal>`
  - `pub fn restore(env: &Env, selectors: &[String]) -> Result<Built, Refusal>`
  - `pub fn scan_data(env: &Env, args: &ScanArgs) -> Result<(serde_json::Value, Vec<ErrorEntry>), Refusal>`. Use the same pattern for `apps_list_data`, `app_show_data`, `orphans_data`, `history_data` and `doctor_data`. Each returns exactly the `data` the `--json` envelope prints today.
  - `select: None` means the CLI's defaults. `Some(paths)` means exactly those findings out of `selectable`. A path not in `selectable` is `Refusal { exit_code: 2, message: "not found by crumbs: <path>" }`.
  - `render::preview_to(w: &mut dyn std::fmt::Write, env: &Env, plans: &[(String, &Plan)], color: bool)`. The existing `render::preview` becomes a wrapper that writes to stdout with colour on.
  - `act::print_preview` is renamed `act::preview_text(env, targets, after_preview, errors) -> String`, which uses no colour. The CLI prints that string, applying colour through the existing path.
  - `pub fn execute_confirmed(env: &Env, cfg: &Config, targets: &[Target], privilege: Privilege) -> ExecOutcome`. It is the body of `apply` after the confirmation: running-app check result included, `before_execute_hook`, signals, `execute_many` and history. `ExecOutcome` holds the per-plan reports, the exit code, and `manual_commands`.
  - `act::apply` calls `execute_confirmed` after its prompts.
- The CLI `run` functions become: build, then print (or refuse), then `act::apply`. Behaviour must be byte-identical.

- [ ] **Step 1: Pin the current output.** Run `cargo test --workspace` and record the counts. No new test is needed for a pure refactor beyond the next step.
- [ ] **Step 2: Write the failing tests**, in a new `crates/crumbs-cli/src/cmd/build.rs` `#[cfg(test)]` module:
  - `build_uninstall_matches_cli_plan`: for a fixture app, `build::uninstall(...)` returns the same `targets[0].plan` (compared via `serde_json::to_value`) as the CLI's `uninstall --dry-run --json` `data.plans[0]`.
  - `build_select_rejects_unknown_path`: `select: Some(&["/etc/passwd"])` gives `Refusal { exit_code: 2, .. }`, with a message starting `not found by crumbs:`.
  - `preview_text_has_no_escape_codes`: `preview_text(...)` contains no `\x1b`.
- [ ] **Step 3: Implement**, moving code without changing behaviour.
- [ ] **Step 4: Run** `cargo test --workspace`. Expected: the same counts plus the new tests, and every snapshot unchanged. Also run `cargo clippy --workspace --all-targets --all-features -- -D warnings`.
- [ ] **Step 5: Commit** `refactor(cli): command builders and string rendering`.

### Task 2: Escalation without a terminal

**Files:**
- Modify:
  - `crates/crumbs-core/src/privilege/batch.rs` (`SystemPrivilegedRunner`)
  - `crates/crumbs-core/src/privilege/mod.rs` (`detect_escalation`)
  - `crates/crumbs-cli/src/cmd/act/privilege.rs`
- Test: in those files.

**Interfaces:**
- Produces:
  - `pub enum Interaction { Terminal, NoTerminal }`
  - `SystemPrivilegedRunner::with_interaction(root: &Path, i: Interaction) -> Self`.
    - Under `NoTerminal`, stdin is `Stdio::null()` and stderr is `Stdio::piped()`. The captured stderr comes back alongside stdout in a new `PrivilegedRun { stdout: Vec<u8>, stderr: Vec<u8> }`, through a new trait method with a default implementation.
    - `Terminal` keeps today's inherit behaviour exactly.
  - `detect_escalation_for(env, runner, i: Interaction)` excludes `sudo` under `NoTerminal`.
  - `Privilege::for_server(env: &Env) -> Privilege`. It applies the same debug-only test-runner gating as `for_run`: `CRUMBS_TEST_PRIV_RUNNER` only when root != "/", and `CRUMBS_ALLOW_REAL_ESCALATION` otherwise. It uses `Interaction::NoTerminal`.

- [ ] **Step 1: Write the failing tests:**
  - `no_terminal_never_picks_sudo`: run0 and pkexec are absent and sudo is present. `detect_escalation_for(.., NoTerminal)` is `None`.
  - `no_terminal_runner_uses_null_stdin`: a `FakePrivilegedRunner` variant records the stdin mode it was asked for. Assert that it is null.
  - `terminal_behaviour_unchanged`: the existing escalation tests still pass.
- [ ] **Step 2: Run** `cargo test -p crumbs-core privilege`. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** `cargo test --workspace`. Expected: PASS.
- [ ] **Step 5: Commit** `feat(core): escalation without a terminal`.

### Task 3: `crumbs mcp` with the read tools

**Files:**
- Modify: `crates/crumbs-cli/Cargo.toml` (feature `mcp`, default on; optional deps per Tech Stack) and `crates/crumbs-cli/src/main.rs` (subcommand `mcp`, `#[cfg(feature = "mcp")]`).
- Create: `crates/crumbs-cli/src/mcp/{mod.rs,server.rs,tools.rs,log.rs}`.
- Test: `crates/crumbs-cli/tests/mcp.rs`, gated `#![cfg(all(feature = "mcp", debug_assertions))]`, plus unit tests in `mcp/`.

**Interfaces:**
- Consumes: the Task 1 `*_data` functions.
- Produces:
  - `mcp::serve(env: Env) -> anyhow::Result<ExitCode>`. It builds a current-thread tokio runtime and serves on stdio until EOF.
  - The tools `doctor`, `apps_list`, `app_show`, `scan`, `orphans` and `history`, with arguments exactly as in spec §2. Each returns structured content equal to the CLI `--json` `data`.
  - A `Refusal` becomes a tool error whose text is `message` and whose structured content is `{ "exit_code": n, "details": [...] }`.
  - The `mcp::log` sink sends warnings as `notifications/message` at level `warning`.
  - Test harness, in `mcp::testing`: an in-process client over `tokio::io::duplex`, built with rmcp's `client` feature as a dev-dependency.

- [ ] **Step 1: Write the failing tests:**
  - `read_tools_match_cli_json`: for each read tool, on a fixture root, the structured content equals the CLI's `--json` `data` for the same arguments.
  - `stdout_carries_only_protocol`:
    1. Spawn the debug binary `crumbs mcp` with `CRUMBS_ROOT=<fixture>`. The fixture has an invalid config key and a bad rule file, so warnings exist.
    2. Write `initialize`, `notifications/initialized` and a `tools/call scan`.
    3. Close stdin.
    4. Every stdout line parses as a JSON-RPC message, and at least one `notifications/message` with level `warning` arrived.
  - `user_and_system_together_is_tool_error`: `scan` with `user: true, system: true` returns `isError` and `exit_code` 2.
  - `no_default_features_builds`: a CI-style check. It is a step, not a test: `cargo build --no-default-features -p crumbs-cli` succeeds.
- [ ] **Step 2: Run** `cargo test -p crumbs-cli --test mcp`. Expected: FAIL.
- [ ] **Step 3: Implement.** Use `#[tool_router]` and `#[tool]` from `rmcp` macros. Argument structs derive `schemars::JsonSchema`.
- [ ] **Step 4: Run** `cargo test --workspace`, then `cargo build --no-default-features -p crumbs-cli`. Expected: both PASS.
- [ ] **Step 5: Commit** `feat(mcp): crumbs mcp with read tools`.

### Task 4: Plan store and preview tools

**Files:**
- Create: `crates/crumbs-cli/src/mcp/plans.rs`.
- Modify: `mcp/tools.rs`.
- Test: `tests/mcp.rs` and `mcp/plans.rs` unit tests.

**Interfaces:**
- Consumes:
  - the Task 1 builders, `Built.cli_command` and `act::preview_text`;
  - `Env`'s clock: use `crumbs_core` time through `Env` so `set_clock_shift` works in tests.
- Produces:
  - `pub struct PlanId(String)`: 32 lowercase hex chars from 16 bytes of `/dev/urandom`.
  - `pub struct StoredPlan { targets, preview_text: String, needs_admin: bool, cli_command: Vec<String>, expires_at: SystemTime }`
  - `pub struct PlanStore`, with these methods:
    - `insert(StoredPlan) -> PlanId`;
    - `peek(&PlanId) -> Option<&StoredPlan>`;
    - `take(&PlanId) -> Option<StoredPlan>`;
    - `purge_expired(now)`, which runs on every tool call.
  - Plan time-to-live: `const PLAN_TTL: Duration = Duration::from_secs(600)`.
  - The tools `preview_uninstall`, `preview_clean`, `preview_orphans` and `preview_restore`, with spec §2 arguments. Each result is the CLI dry-run `data` (`plans` plus `report: null`) with these extra fields:
    - `plan_id`;
    - `expires_at` (RFC 3339, UTC);
    - `needs_admin`;
    - `preview_text`;
    - `cli_command`, a single shell-quoted string.
  - A plan with no actions returns `plan_id: null` and stores nothing.

- [ ] **Step 1: Write the failing tests:**
  - `preview_returns_plan_and_stores_it`: `preview_uninstall` on a fixture app returns a 32-hex `plan_id`, and `preview_text` equals `act::preview_text` for the same targets.
  - `select_rejects_unknown_paths`: `preview_uninstall` with `select: ["/etc/passwd"]`, then with `["~/.ssh"]`. Each gives a tool error, `exit_code` 2, and nothing stored.
  - `system_leftovers_need_explicit_select`: without `select`, no SystemTree action is in the plan. With `select` naming `/opt/inky`, the plan has it and `needs_admin` is true.
  - `expired_plan_is_purged`: insert, then `set_clock_shift(+601 s)` and call any tool. `peek` is `None`.
  - `plan_ids_are_unique`: 1000 inserts give 1000 distinct ids.
- [ ] **Step 2: Run** them. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** `cargo test --workspace`. Expected: PASS.
- [ ] **Step 5: Commit** `feat(mcp): plan store and preview tools`.

### Task 5: `apply` with the user's confirmation

**Files:**
- Create: `crates/crumbs-cli/src/mcp/confirm.rs`.
- Modify: `mcp/tools.rs` and `mcp/server.rs`.
- Test: `tests/mcp.rs`.

**Interfaces:**
- Consumes:
  - `PlanStore` (Task 4);
  - `execute_confirmed` (Task 1);
  - `Privilege::for_server` (Task 2);
  - from rmcp: `Peer<RoleServer>::{protocol_version, supported_elicitation_modes, elicit_with_timeout}`, `InputRequiredResult`, `ElicitRequest` and `InputResponses`.
- Produces:
  - `#[derive(Deserialize, JsonSchema)] struct Confirm { confirm: bool }` with `rmcp::elicit_safe!(Confirm)`.
  - Confirmation message: `const CONFIRM_SUFFIX: &str = "\n\nApply this plan?"`. The message is `preview_text + CONFIRM_SUFFIX`.
  - `CONFIRM_TIMEOUT: Duration = 600 s` (test-overridable via a debug-only `CRUMBS_TEST_CONFIRM_TIMEOUT_MS`, honoured only when root != "/").
  - Result shapes:
    - `{ "applied": true, "report": [...], "exit_code": n }`, with `isError` when n == 3;
    - `{ "applied": false, "reason": "declined" | "cancelled" | "timed out" }`;
    - `{ "applied": false, "exit_code": 5, "manual_commands": [...] }`.
  - The apply mutex (Decision 6) is held from the confirmation round through execution.

- [ ] **Step 1: Write the failing tests.** The test client's elicitation handler is scripted per test.
  - `apply_confirmed_runs_plan_and_writes_history`: answer accept with `confirm: true`. Afterwards the fixture file is in the trash, history has the entry, and `applied` is true.
  - `apply_without_confirmation_changes_nothing`: run three cases (decline, cancel, no elicitation capability). For each, the file is still in place and history is unchanged. The no-capability case's error text contains `crumbs uninstall`.
  - `apply_rejects_accept_without_confirm_true`: answer accept with `confirm: false`, then accept with empty content. Nothing runs.
  - `plan_id_is_single_use`: after a confirmed apply, a second `apply` with the same id gives `plan not found or expired; preview again`.
  - `apply_refuses_swapped_target`:
    1. Preview.
    2. Replace the fixture dir with a new inode.
    3. Confirm.
    4. The report shows the action refused, and the new dir is still there.
  - `unanswered_confirmation_times_out_and_drops_plan`:
    1. Set `CRUMBS_TEST_CONFIRM_TIMEOUT_MS=200` and have the handler never answer.
    2. The result reason is `timed out`.
    3. A second preview and apply can take the mutex.
    4. The old id is gone.
  - `mrtr_flow_requires_matching_state`:
    1. A client negotiating `2026-07-28` gets `InputRequiredResult` on the first call.
    2. A retry with the right `requestState` and a `confirm: true` response runs.
    3. A retry with a different `requestState`, or a response to another request id, runs nothing.
  - `privileged_plan_uses_one_batch_and_dev_null_stdin`: a plan with a SystemTree delete, using the `sh` test runner. One batch runs, and the runner saw stdin null.
- [ ] **Step 2: Run** them. Expected: FAIL.
- [ ] **Step 3: Implement.** Remove the plan only as Decision 3 says. Call `execute_confirmed` inside `spawn_blocking`.
- [ ] **Step 4: Run** `cargo test --workspace`, then `RAYON_NUM_THREADS=1 cargo test --workspace`. Expected: PASS.
- [ ] **Step 5: Commit** `feat(mcp): apply with the user's confirmation`.

### Task 6: Docs

**Files:**
- Modify:
  - `README.md`: a new section "Using crumbs from an AI agent". It covers what agents can and cannot do, the confirmation, admin prompts and `crumbs mcp`, plus client config snippets. For Claude Code: `claude mcp add crumbs -- crumbs mcp`. For Claude Desktop: a `claude_desktop_config.json` `mcpServers` entry `{ "command": "crumbs", "args": ["mcp"] }`.
  - `docs/json-schema.md`: the MCP tool results, the preview extras, and the apply result shapes.
  - Spec §13: 4c done.
- Create: `docs/superpowers/notes/2026-11-08-m4c-review-followups.md`.

- [ ] **Step 1: Write the docs.** Generate the tool-result examples from the in-process test client against a fixture.
- [ ] **Step 2: Cross-check them** against the serde structs and tool schemas (`tools/list` output).
- [ ] **Step 3: Commit** `docs: crumbs mcp`.
