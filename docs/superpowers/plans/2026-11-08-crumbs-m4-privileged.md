# crumBS Milestone 4 (Privilege, Package Removal, System Cleanup) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Three new capabilities, each going through the same preview → confirm → act → history pipeline with exactly one privilege prompt per run:
- `crumbs uninstall` fully removes pacman and Flatpak apps (package and leftovers);
- `crumbs scan --system` reports system-level bloat;
- `crumbs clean --system` removes it.

**Architecture:**
- **Core building blocks:**
  - a `privilege::batch` module, which builds one shell script from the privileged actions, runs it through the detected escalation (run0 → pkexec → sudo) and maps `@@crumbs <i> <status>` markers back to actions;
  - two new actions, `PrivilegedCommand` and `PackageRemove`;
  - a new safety scope, `Scope::System`;
  - per-source removal planning with simulation (`pacman -Rs --print`).
- **Executor order:** the privileged batch runs first. User-level leftover actions depend on the package action that removes their app.
- **CLI:** `uninstall` drops the `--leftovers-only` requirement for pacman and Flatpak apps, and `scan` and `clean` gain the six system categories.

**Tech Stack:** The M3 stack. No new dependencies; shell-escaping is done in-crate.

**Spec:** `docs/superpowers/specs/2026-11-08-crumbs-design.md` §3 (removal column), §5 (system scope), §6, §7, §8, §13 milestone 4. M4 entry criteria are in:
- `docs/superpowers/notes/2026-11-08-m1-review-followups.md`
- `docs/superpowers/notes/2026-11-08-m2-review-followups.md`
- `docs/superpowers/notes/2026-11-08-m3-review-followups.md`

## Decisions this plan makes (the spec leaves them open)

1. **Scope split.** M4 covers:
   - pacman (Arch and Manjaro) package removal;
   - Flatpak app removal for user, system and custom installations;
   - the six system categories (`pkg-cache`, `orphan-pkgs`, `journal`, `coredumps`, `system-temp`, `flatpak-unused-system`).

   The rest is split off:
   - apt and dnf variants of the package categories move to M5, together with their sources;
   - system leftovers (`/etc/<name>`, `/opt/<name>` not owned by any package) and path-based removal of unmanaged apps move to a new **M4b**, together with their owned-directory database. They are the riskiest deletions and need their own plan.

   On a system without pacman, the pacman-only categories report `unavailable` and are not an error.
2. **One prompt, one script.**
   - All privileged actions in a run are joined into one `/bin/sh` script and run with a single escalation: `run0 --` → `pkexec` → `sudo` (sudo only on a TTY).
   - The script starts with `export PATH=/usr/bin:/usr/sbin:/bin:/sbin LC_ALL=C; unset LD_PRELOAD LD_LIBRARY_PATH`.
   - Each command runs as `<absolute trusted program> <single-quoted args> </dev/null 2>&1; printf '@@crumbs %d %d\n' <i> $?`.
   - The escalation process inherits the terminal's stdin and stderr, so polkit, run0 and sudo can prompt. Its stdout is captured and parsed.
3. **Escalation failure → exit 5.**
   - No escalation tool, or a batch that produced no markers (cancelled or denied): every privileged action gets `ok: false` (`privilege escalation was cancelled or denied`, or `no way to get administrator rights`).
   - Actions that depend on them are skipped, the CLI prints the exact commands for manual use, and the exit code is 5.
4. **Simulate first, verify after.**
   - pacman removals are simulated as the user with `pacman -Rs --print --print-format '%n %v' <pkgs>`. pacman refuses `--print` together with `-n`; the real command is `pacman -Rns --noconfirm <pkgs>`.
   - A simulation error ("breaks dependency") refuses the app (exit 1) with pacman's message.
   - Extra packages from the simulation are listed under `also removed` in the preview.
   - After the batch, each `PackageRemove` is confirmed by re-reading the local pacman database. A package still present means `ok: false`, `still installed`.
   - Before anything runs, a `/var/lib/pacman/db.lck` refuses the run with `pacman is busy (database lock present)`.
5. **Flatpak apps.**
   - User installations are removed as a `UserCommand`: `flatpak uninstall --user -y --noninteractive <id>`.
   - System and custom installations are removed as a `PrivilegedCommand`: `flatpak uninstall --system|--installation=<name> -y --noninteractive <id>`.
   - `--delete-data` is **not** used, so `~/.var/app/<id>` is trashed by crumbs as a normal, restorable leftover.
6. **Opt-in system categories.** `orphan-pkgs` and `flatpak-unused-system` are listed but not pre-selected, and run only when named, like M3's `flatpak-unused` and `trash`. Orphaned packages are often optional dependencies the user wants.

   Pre-selected under `--system`: `pkg-cache`, `journal`, `coredumps` and `system-temp`.
7. **`clean` / `scan` flags.**
   - With no flag, they cover user categories only (M3 behaviour).
   - `--system` covers system categories only.
   - Naming a system category implies `--system` for that category.
   - `--user --system` is a usage error (exit 2) until a later milestone.
8. **`Scope::System` (used by `coredumps` only in M4).** A target must be a direct child regular file of in-system `/var/lib/systemd/coredump` whose name starts with `core.`, and must not be a symlink. Every directory from `/` down to that directory must be root-owned and not group- or other-writable. Deletion is a `PrivilegedCommand` `rm -f -- <path>`, with the target verified, as the user, immediately before the batch runs.
9. **History.**
   - New history actions: `package-remove` (stores `names` with versions, and `source`) and `privileged-command` (stores `argv`). Neither can be restored.
   - Restore reports `packages cannot be restored by crumbs; reinstall with: pacman -S <names>`. Batch restores skip these entries, like M3 commands.

## Global Constraints

- All M1–M3 global constraints still apply.
- **No new dependencies.**
- **Never trigger a real privilege escalation in tests or verification:**
  - Tests use a `FakePrivilegedRunner`, which runs the generated script with `/bin/sh` as the test user against fixture programs.
  - Real-system checks are limited to `--dry-run`, `scan --system` and simulations (`pacman -Rs --print`, `paccache -d`, `journalctl --disk-usage`), all of which run as the user and change nothing.
  - **Never** run `uninstall` of a pacman or Flatpak app, or `clean --system`, without `--dry-run` on the real system.
- **Inputs to the script:**
  - Every argument is single-quote escaped by one function, `privilege::batch::sh_quote`.
  - Package names must match `^[A-Za-z0-9@._+-]+$`, and a name failing that is refused before planning.
  - Program paths come only from `resolve_trusted` (root-joined), translated to in-system absolute paths for the script.
- **`SystemRunner` hardening:**
  - Children get a cleared environment plus an allowlist: `HOME USER LOGNAME XDG_RUNTIME_DIR XDG_CONFIG_HOME XDG_DATA_HOME XDG_CACHE_HOME XDG_STATE_HOME DBUS_SESSION_BUS_ADDRESS`, with `PATH=/usr/bin:/usr/sbin:/bin:/sbin` and `LC_ALL=C`.
  - Queries time out after 60 s; `UserCommand` runs time out after 30 min. A timeout gives `CmdOutput { timed_out: true, status: -1 }`.
  - The privileged batch has no timeout, but Ctrl-C is forwarded.
- **Config keys:** `pkg_cache_keep = 2` (minimum 0) and `journal_max = "500M"` (a `journalctl` size string matching `^[0-9]+[KMGT]?$`; invalid → warning and default).
- **Category names (kebab-case):** `pkg-cache`, `orphan-pkgs`, `journal`, `coredumps`, `system-temp`, `flatpak-unused-system`.

## Review Focus

1. **The escalation prompt is dismissed.** No package counts as removed, leftovers are skipped through `depends_on`, history records the failures, the exact commands are printed, and the exit code is 5. Pinned in Task 4.
2. **The simulation pulls in extra packages.** The preview lists them under `also removed` before the confirmation. If the database changes before execution, the post-check reports what really happened. Pinned in Tasks 5 and 6.
3. **pacman is busy.** A `db.lck` refuses before any prompt. Pinned in Task 5.
4. **Injection through names or paths.** A package name like `a'b;rm -rf /` is refused by validation. A path containing `'`, a space or a newline is quoted correctly: the script test shows the fixture program receives it as exactly one argument. Pinned in Task 2.
5. **A swapped coredump.** A core file replaced by a symlink between scan and batch is refused by the pre-batch verify, and is never deleted through the link. Pinned in Tasks 3 and 9.

---

## File Structure

```
crates/crumbs-core/src/privilege/mod.rs       # (move privilege.rs here) Escalation detection
crates/crumbs-core/src/privilege/batch.rs     # sh_quote, Script, PrivilegedRunner, SystemPrivilegedRunner, parse markers
crates/crumbs-core/src/removal/mod.rs         # RemovalPlan, plan_removal(app) per source
crates/crumbs-core/src/removal/pacman.rs      # simulate, lock check, name validation, post-check
crates/crumbs-core/src/removal/flatpak.rs     # installation-aware uninstall commands
crates/crumbs-core/src/bloat/system/{mod.rs,pkgcache.rs,orphanpkgs.rs,journal.rs,coredumps.rs,systemp.rs,flatpak.rs}
crates/crumbs-cli/src/cmd/manual.rs           # print commands for manual use (exit 5)
```

Modify: `command.rs`, `safety.rs`, `plan.rs`, `executor/mod.rs`, `history.rs`, `restore.rs`, `config.rs`, `bloat/mod.rs`, `sources/flatpak.rs`, `doctor.rs`, `trash.rs`, the CLI's `cmd/{uninstall,clean,scan,act,render,doctor,history,restore}.rs`, `tests/cli.rs`, docs.

---

### Task 1: Runner hardening and type clean-ups (M2/M3 entry criteria)

**Files:** Modify `command.rs`, `safety.rs`, `plan.rs` and every caller of `Action::path()`, plus `testing.rs` (FakeRunner gains timed-out outputs).

**Interfaces:**
- `CmdOutput` gains `pub timed_out: bool`.
- `CommandRunner::run` keeps its signature. `SystemRunner` applies the env allowlist and a 60 s timeout. Add `SystemRunner::with_timeout(Duration)` for `UserCommand` (30 min).
- Implement the timeout with a `wait` thread plus `kill`, std only: spawn, then `try_wait` in a loop with short sleeps, or a waiter thread with a channel.
- `Fingerprint` gains `pub ctime: i64, pub ctime_nsec: i64`, and `verify` compares them. This catches inode reuse.
- `Action::path()` becomes `-> Option<&Path>`, returning `None` for `UserCommand`. Update every caller: `drop_overlaps`, `skip_reason`, render and restore.

- [ ] **Step 1: Failing tests:**
  - `system_runner_clears_env`: a fixture script prints `env`. `LD_PRELOAD` and `FOO` are absent; `PATH` and `LC_ALL` are as specified; `HOME` is kept.
  - `system_runner_times_out`: `sleep 5` with a 200 ms timeout gives `timed_out`.
  - `fingerprint_detects_inode_reuse`: replace a file and force the same inode if the filesystem allows it. Otherwise test the comparison function directly with two fingerprints that differ only in ctime.
  - `action_path_none_for_commands`
- [ ] **Step 2: Run** and expect FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** the workspace and expect PASS. Commit `fix(core): hardened runner env and timeouts; ctime in fingerprints; command actions have no path`.

---

### Task 2: Privileged batch script and runner

**Files:** Create `privilege/mod.rs` (move `privilege.rs`) and `privilege/batch.rs`. Extend `testing.rs` with `FakePrivilegedRunner`.

**Interfaces:**
- `pub fn sh_quote(s: &OsStr) -> String`. It wraps the argument in single quotes and writes each `'` as `'\''`. It works on raw bytes, so non-UTF-8 survives through `OsStrExt`; the script is built as `Vec<u8>`.
- `pub struct Step { pub program: PathBuf /* in-system absolute */, pub args: Vec<OsString> }`
- `pub fn build_script(steps: &[Step]) -> Vec<u8>` writes the header from Decision 2, then one line per step, with step `i` printing `@@crumbs i $?`.
- `pub struct StepOutcome { pub status: Option<i32> /* None = no marker */, pub output: String /* lossy, the lines before its marker */ }`
- `pub fn parse_markers(stdout: &[u8], steps: usize) -> Vec<StepOutcome>`. Lines that look like markers but have an out-of-range index or a non-numeric status are treated as output.
- `pub trait PrivilegedRunner: Send + Sync { fn run(&self, esc: &Escalation, script: &[u8]) -> Result<Vec<u8> /* stdout */, CoreError>; }`
- `pub struct SystemPrivilegedRunner;`
  - Runs `<esc.program> [--] /bin/sh -c <script>`:
    - `run0 --`: argv `[run0, "--", "/bin/sh", "-c", script]`;
    - `pkexec`: `[pkexec, "/bin/sh", "-c", script]`;
    - `sudo`: `[sudo, "--", "/bin/sh", "-c", script]`.
  - stdin and stderr are inherited and stdout is piped. The process environment is the same allowlist as `SystemRunner`, plus `DISPLAY`, `WAYLAND_DISPLAY` and `XAUTHORITY` so the polkit agent can prompt.
- (test-support) `pub struct FakePrivilegedRunner { pub calls: Mutex<Vec<Vec<u8>>> }` runs `/bin/sh -c <script>` as the test user and records every script.

- [ ] **Step 1: Failing tests:**
  - `sh_quote_table`: plain text, the empty string, `'`, a space, a newline, `$(x)`, and a non-UTF-8 byte.
  - `script_runs_each_step_with_exact_args` (Review Focus #4): a fixture program writes its argv as NUL-separated output to a file, and an arg containing `a'b c\nd $(rm -rf /)` arrives as one exact argument.
  - `markers_parsed_and_output_attributed`
  - `missing_marker_is_none`: a step that kills the shell.
  - `forged_marker_in_output_ignored`: a program prints `@@crumbs 7 0` with only 2 steps.
  - `header_clears_preload`
- [ ] **Step 2: Run** and expect FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** and expect PASS. Commit `feat(core): privileged batch script, runner and marker parsing`.

---

### Task 3: `Scope::System` (Decision 8)

**Files:** Modify `safety.rs`.

**Interfaces:**
- `Scope::System`.
- `is_allowed_target(.., Scope::System, path)` holds when all of these are true:
  - the path is canonical;
  - its parent is in-system `/var/lib/systemd/coredump`;
  - its name starts with `core.`;
  - it is not protected or excluded.
- `verify_now`/`verify` for System: lstat the target, which must be a regular file and not a symlink. Then check every directory from `/` to the parent: each must be a real directory, `uid == 0`, and `mode & 0o022 == 0`. Then compare the fingerprint.
- Root ownership uses `env.tmp_owner_uid`-style injection for tests: rename the hook to `env.system_owner_uid` (default 0) and reuse it for `/tmp`, keeping a single hook.

- [ ] **Step 1: Failing tests:**
  - `system_scope_allows_core_files_only`
  - `system_scope_refuses_symlink_target` (Review Focus #5)
  - `system_scope_refuses_writable_parent`
  - `system_scope_refuses_non_root_parent`
- [ ] **Step 2: Run** and expect FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** and expect PASS. Commit `feat(core): system safety scope for coredumps`.

---

### Task 4: Executor and actions for privilege (Decisions 2, 3, 9)

**Files:** Modify `plan.rs`, `executor/mod.rs`, `history.rs`, `restore.rs` and `privilege/mod.rs`.

**Interfaces:**
- `Action::PrivilegedCommand { program: String, args: Vec<String> }`, tag `privileged-command`.
- `Action::PackageRemove { source: SourceKind, names: Vec<String>, versions: Vec<String>, program: String, args: Vec<String> }`, tag `package-remove`. `program`/`args` is the exact removal command, built by `removal` (Task 5).
- `pub fn execute_full(env, cfg, plan, runner: &dyn CommandRunner, priv_runner: &dyn PrivilegedRunner, esc: Option<&Escalation>) -> ExecReport`:
  1. Collect the privileged actions (`PrivilegedCommand`, `PackageRemove`, and `Delete` in `Scope::System`). The executor sends a System-scope `Delete` to the batch as `rm -f -- <path>`; plans keep it as a plain `Delete` carrying its scope and fingerprint.
  2. Verify each System-scope Delete (as the user) right before the batch. Failures get `ok: false` and are left out of the script.
  3. Build one script and run it once.
  4. Map the outcomes onto results:
     - a marker with status 0 → ok;
     - a non-zero status → `ok: false` with the last output line;
     - no marker → `privilege escalation was cancelled or denied`.
  5. If `esc` is `None`: every privileged action gets `ok: false`, `no way to get administrator rights (install systemd ≥ 256, polkit or sudo)`, and `ExecReport.escalation_failed = true`.
  6. Run the remaining actions in plan order with the existing per-action logic (dependency skips use the privileged results).
  7. Append history per action, as now. `PackageRemove` entries store `names` and `versions` (new optional `packages` field on `HistoryEntry`).
- `ExecReport` gains `pub escalation_failed: bool`. It is true when no marker came back for at least one privileged step, or there was no escalation.
- `execute`/`execute_with` keep working: they call `execute_full` with the system runners and `detect_escalation`.
- **Post-check hook:** `PackageRemove` results are re-checked by a callback `removal::still_installed(env, source, names) -> Vec<String>` (Task 5 provides it; this task takes a `&dyn Fn` and defaults to "no post-check").
- **Restore:** `PackageRemove` gives `packages cannot be restored by crumbs; reinstall with: pacman -S <names>`, and `PrivilegedCommand` gives `commands cannot be undone`. Batch restores skip both.

- [ ] **Step 1: Failing tests** (`FakePrivilegedRunner` and fixture programs):
  - `privileged_actions_run_in_one_batch`: one script containing 3 steps.
  - `escalation_denied_marks_all_failed_and_skips_dependents` (Review Focus #1): a fake runner returns empty stdout.
  - `no_escalation_tool_reports_and_flags`
  - `system_delete_verified_before_batch`: a swapped core file is excluded from the script.
  - `package_remove_post_check_still_installed`
  - `history_records_packages`
  - `restore_refuses_packages_with_reinstall_hint`
- [ ] **Step 2: Run** and expect FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** the workspace and expect PASS. Commit `feat(core): privileged batch execution, package-remove and privileged-command actions`.

---

### Task 5: Removal planning (Decisions 4, 5)

**Files:** Create `removal/mod.rs`, `removal/pacman.rs` and `removal/flatpak.rs`. Modify `sources/flatpak.rs` so it records the installation name (a custom installation gets `Installation::Custom(String)`; serde gives `custom:<name>`), and `model.rs`.

**Interfaces:**
- `pub struct RemovalPlan { pub actions: Vec<Action>, pub also_removed: Vec<(String, String)> /* name, version */, pub notes: Vec<String> }`
- `pub enum RemovalError { Busy(String), Refused(String) /* simulation error text */, Unsupported(String), InvalidName(String) }` (thiserror, Display = message)
- `pub fn plan_removal(env, runner, app: &App) -> Result<RemovalPlan, RemovalError>`:
  - **pacman:**
    1. Validate the name.
    2. Check for `db.lck`, which gives `Busy`.
    3. Run `pacman -Rs --print --print-format '%n %v' <name>` through the runner. A non-zero exit gives `Refused` with the trimmed stderr (the "breaks dependency" lines).
    4. On success, return `PackageRemove { names: [name] + extras, versions, command: ("pacman", ["-Rns","--noconfirm", names…]) }`, with `also_removed` holding every name other than the app's own.
  - **Flatpak:** user → `UserCommand flatpak uninstall --user -y --noninteractive <id>`; system → `PrivilegedCommand flatpak uninstall --system -y --noninteractive <id>`; custom → `--installation=<name>`.
  - **AppImage:** unchanged (M2 path).
  - **Unmanaged:** `Unsupported("removing unmanaged apps needs M4b; use --leftovers-only")`.
- `pub fn still_installed(env, source, names) -> Vec<String>` re-reads `/var/lib/pacman/local` for pacman. For Flatpak it checks `flatpak list` and is used only for system or custom installations.

- [ ] **Step 1: Failing tests:**
  - `pacman_simulation_lists_extras` (Review Focus #2)
  - `pacman_simulation_refusal_text`
  - `pacman_busy_lock` (Review Focus #3)
  - `invalid_package_name_refused`
  - `flatpak_user_system_custom_commands`
  - `custom_installation_parsed` (the installation column with a custom name)
  - `still_installed_reads_db`
- [ ] **Step 2: Run** and expect FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** and expect PASS. Commit `feat(core): removal planning with pacman simulation and flatpak installations`.

---

### Task 6: CLI full uninstall and escalation UX

**Files:** Modify `cmd/uninstall.rs`, `cmd/act.rs`, `cmd/render.rs` and `cmd/doctor.rs`. Create `cmd/manual.rs`. Modify `tests/cli.rs`.

**Behaviour:**
- **Who needs `--leftovers-only`:** pacman and Flatpak apps no longer require it. A full uninstall uses `plan_removal` plus the M2 leftovers, and every leftover action `depends_on` the package or command action. Unmanaged apps still require it.
- **Refusals:** `RemovalError::Busy` and `Refused` exit 1 with the message, and under `--json` the envelope's `errors` carries it with source `cli`. `InvalidName` also exits 1, and `Unsupported` exits 2.
- **Preview:**
  - a `remove` row per package action: `remove  —  pacman: firefox 159.0.1-1`;
  - an `also removed: gn 0.2385…, libjuice 1.7.3-1` line;
  - `run` rows for commands, showing the full command line;
  - a note `packages are removed with administrator rights; you will be asked once` when any privileged action exists;
  - a note `this cannot be undone by crumbs` for package removals.
- **Escalation:**
  - Detect with `detect_escalation` before confirming.
  - If there is none and privileged actions exist, the preview still shows. `--dry-run` exits 0; otherwise crumbs exits 5 after printing `no way to get administrator rights` and the manual commands (`cmd/manual.rs`, one line per command, shell-quoted, prefixed by `sudo`).
  - `requires_tty` with a non-TTY stdin is treated as no escalation.
- **Report:** after execution with `escalation_failed`, print `administrator rights were not granted; nothing that needs them was changed`, the manual commands, and exit 5. Package rows show `✓ removed pacman: firefox` / `✗ … still installed`.
- **`doctor`:** add a line `privileged cleanup: available via run0` (or `unavailable`).
- **Exit when every source failed (M1 follow-up):** `apps list` and `doctor` exit 1, not 3, when every available source failed.

- [ ] **Step 1: Failing tests** (fixture with a pacman db, fake `pacman` and `run0` scripts, `FakePrivilegedRunner` via a hidden debug-only env var `CRUMBS_TEST_PRIV_RUNNER=sh` that makes the CLI use the fake runner; this exists only under `cfg(debug_assertions)`):
  - `uninstall_pacman_dry_run_shows_also_removed`
  - `uninstall_pacman_executes_batch_and_trashes_leftovers`
  - `uninstall_pacman_refused_by_simulation_exit_1`
  - `uninstall_pacman_busy_exit_1`
  - `uninstall_without_escalation_exit_5_prints_manual_commands`
  - `uninstall_escalation_denied_exit_5_leftovers_untouched`
  - `uninstall_flatpak_user_runs_command`
  - `uninstall_flatpak_system_uses_batch`
  - `doctor_shows_privileged_availability`
  - `all_sources_failed_exit_1`
- [ ] **Step 2: Run** and expect FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** the workspace and expect PASS. Real-system check (read-only): `cargo run -q -p crumbs-cli -- uninstall <some pacman GUI app> --dry-run`. Pick one without dependents, such as `kate` if installed; otherwise any pacman app from `apps list`. Paste the preview. **Never run it without `--dry-run`.**
- [ ] **Step 5: Commit** `feat(cli): full uninstall for pacman and flatpak apps with one privilege prompt`.

---

### Task 7: System scanners (Decisions 6, 8)

**Files:** Create the `bloat/system/*` files. Modify `bloat/mod.rs` (`Category` gains the six variants and `fn system(self) -> bool`, and `ALL` order appends them), `config.rs` (`pkg_cache_keep`, `journal_max`) and `ItemAction` (adds `Privileged { program, args }`).

**Behaviour:**
- **`pkg-cache`** (needs pacman):
  - If `paccache` resolves, run `paccache -dk<keep>` and parse `disk space saved: <size>` (IEC suffixes KiB, MiB, GiB) into the size. The item is `Privileged paccache -rk<keep>`.
  - Otherwise use `pacman -Sc --noconfirm` with size `None` and the note `keeps only installed versions (pacman-contrib not installed)`.
  - No candidates means no item.
- **`orphan-pkgs`** (opt-in): `pacman -Qdtq` gives the names. Simulate with `pacman -Rs --print --print-format '%n %v' <names>`; the size is the sum of `%SIZE%` from the local db. The single item is `Privileged pacman -Rns --noconfirm <names>` with label `N orphaned packages`, and its note lists the first 5 names followed by `…`. A simulation failure becomes a scanner `Err` with pacman's message.
- **`journal`:** run `journalctl --disk-usage` and parse `take up <size>`. If usage is above `journal_max`, the item is `Privileged journalctl --vacuum-size=<journal_max>` with size `usage − max`. Otherwise there is no item.
- **`coredumps`:** one item per `core.*` file in `/var/lib/systemd/coredump`, with size from lstat, `Scope::System`, `ItemAction::Delete` and fingerprint. The plan turns these into `PrivilegedCommand rm -f -- <path>` (Task 8).
- **`system-temp`:** if `systemd-tmpfiles` resolves, one item `Privileged systemd-tmpfiles --clean` with size `None`.
- **`flatpak-unused-system`** (opt-in): if `flatpak list --system --runtime --columns=ref` is non-empty, one item `Privileged flatpak uninstall --system --unused -y --noninteractive`.
- **Availability:** a missing tool makes the category unavailable: `Ok(vec![])`, and the CategoryReport NOTE says `unavailable (<tool> not installed)`. Add `pub unavailable: Option<String>` to `CategoryReport`, with serde skipping `None`.

- [ ] **Step 1: Failing tests** (fixture programs that print realistic output; use the real lines captured on the dev machine: `==> finished dry run: 1 candidates (disk space saved: 86.17 MiB)` and `Archived and active journals take up 719.1M in the file system.`):
  - `pkg_cache_parses_dry_run`
  - `pkg_cache_without_paccache`
  - `orphan_pkgs_item_and_opt_in`
  - `orphan_pkgs_simulation_error`
  - `journal_over_limit`
  - `journal_under_limit_no_item`
  - `coredumps_items_system_scope`
  - `system_temp_item`
  - `flatpak_unused_system_opt_in`
  - `unavailable_category_note`
- [ ] **Step 2: Run** and expect FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** and expect PASS. Commit `feat(core): system bloat scanners`.

---

### Task 8: `scan --system` and `clean --system` (Decision 7)

**Files:** Modify `cmd/scan.rs`, `cmd/clean.rs` and `plan.rs` (`build_clean_plan` maps `ItemAction::Privileged` to `PrivilegedCommand`; a System-scope `Delete` stays a `Delete` with its scope and fingerprint, and the executor turns it into a batch step per Task 4), and `tests/cli.rs`.

**Behaviour:**
- **Flags:** `--system` selects the system categories. Naming one implies it. `--user --system` exits 2 with `use either --user or --system`. Remove M3's `--system` "not available" message.
- **Pre-selection and refusals:** use the opt-in rules from Decision 6 and M3's named-category rules. Escalation detection, exit 5, manual commands and report lines are the same as in Task 6. A preview with privileged actions shows the "asked once" note.
- **Scan:** `scan --system` is read-only and needs no escalation. Unavailable categories show their note.

- [ ] **Step 1: Failing tests:**
  - `scan_system_table`
  - `scan_user_and_system_conflict_exit_2`
  - `clean_system_dry_run_preview`
  - `clean_system_runs_one_batch`
  - `clean_system_orphan_pkgs_opt_in`
  - `clean_system_escalation_denied_exit_5`
  - `clean_system_coredump_swapped_refused` (Review Focus #5, end to end)
  - `clean_default_still_user_only`
- [ ] **Step 2: Run** and expect FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** the workspace and expect PASS. Real-system checks, read-only: `cargo run -q --release -p crumbs-cli -- scan --system` and `… clean --system --dry-run`. Paste both. **Never run without `--dry-run`.**
- [ ] **Step 5: Commit** `feat(cli): scan and clean system categories`.

---

### Task 9: Remaining hardening (entry criteria)

**Files:** Modify `history.rs`, `trash.rs`, `executor/mod.rs` (`running_processes`), `cmd/act.rs` and `env.rs`.

**Behaviour:**
- **History file:**
  - It is opened with `O_NOFOLLOW`, and a symlink gives an error.
  - If the history dir or file exists with mode bits beyond `0o700`/`0o600`, those modes are tightened, with a `warning:`.
  - A torn last line without `\n` gets a newline before the next append.
- **`trash::topdir_for`:** the walk is clamped at `env.root`, which matters for fake roots.
- **`running_processes`:**
  - strip ` (deleted)` from exe targets;
  - also match the basename of `/proc/<pid>/cmdline` argv0 and of `/proc/<pid>/comm`;
  - never match on the generic basenames `sh bash env python python3 node electron java flatpak bwrap`.
- **Cross-plan gate (M3 follow-up):** in a multi-app uninstall, an action dropped by `drop_overlaps` because an earlier target covers it now records `depends_on_plan: (plan index, action index)`. The CLI skips that dependent if the covering action failed. Implement this in `cmd/act.rs` by carrying the earlier plans' results into later executions.
- **Env test hooks:** `Env.clock_shift` and the root-owner hook become `pub(crate)` with setters available only under `cfg(any(test, feature = "test-support", debug_assertions))`.

- [ ] **Step 1: Failing tests:**
  - `history_refuses_symlink`
  - `history_tightens_modes`
  - `history_repairs_torn_line`
  - `topdir_clamped_at_root`
  - `running_detects_deleted_exe_and_comm`
  - `generic_basenames_never_match`
  - `cross_plan_gate_skips_when_cover_failed`
- [ ] **Step 2: Run** and expect FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** the workspace and expect PASS. Commit `fix: history file hardening, process matching, cross-plan dependency gate`.

---

### Task 10: Docs

**Files:** Modify `docs/json-schema.md`, `README.md`, and spec §13 (milestone 4 done; add `4b. System leftovers and unmanaged removal` and move the M4b items there). Create `docs/superpowers/notes/m4b-scope.md`, listing the deferred items with their M1–M3 follow-up references.

- **JSON schema:** document `package-remove`, `privileged-command`, the history `packages`, `escalation_failed`, `unavailable`, the six system categories, `Installation::Custom` serialization, and exit 5 with the manual-commands output.
- **README:** status M4, full uninstall, one prompt, what needs administrator rights, the opt-in system categories, `pkg_cache_keep`/`journal_max`, and a Safety paragraph covering simulation, the lock check, the post-check, quoting and never `--delete-data`.
- **Examples:** generate them with the debug binary against a fixture, using the fake privileged runner. Never run escalation for real.

- [ ] **Step 1: Write the docs.**
- [ ] **Step 2: Cross-check them** against the serde structs.
- [ ] **Step 3: Commit** `docs: privileged removal and system cleanup`.
