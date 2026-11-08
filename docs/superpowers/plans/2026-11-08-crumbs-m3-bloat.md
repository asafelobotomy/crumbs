# crumBS Milestone 3 (Bloat, User Level) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `crumbs scan` reports reclaimable user-level space by category in under 10 s, and `crumbs clean` removes the user's selection with the same preview → confirm → verify → history → restore pipeline as `uninstall`.

**Architecture:** A new `crumbs-core::bloat` module defines `Category`, `BloatItem` and a `Scanner` trait (spec §5). There is one scanner file per category, and a registry that runs the enabled scanners in parallel. Items become a `Plan` through `plan::build_clean_plan`. The executor gains one new action, `UserCommand`, plus per-action safety *scopes*, so targets outside home (`/tmp`, trash entries) are verified against their own allowed root instead of "under home". The CLI adds `scan` and `clean`, and reuses `cmd/context.rs`, `cmd/act.rs` and `cmd/render.rs`.

**Tech Stack:** The M2 stack plus `rayon`, the only new dependency.

**Spec:** `docs/superpowers/specs/2026-11-08-crumbs-design.md`: section 5 (user scope), sections 6, 8, 9 and 10, and milestone 3 of section 13. The M3 entry criteria are in `docs/superpowers/notes/2026-11-08-m2-review-followups.md`, and M2's decisions are in `docs/superpowers/notes/2026-11-08-m2-rulings.md`.

## Decisions this plan makes (the spec leaves them open)

1. **Trash first, with two exceptions.** `clean` trashes items by default, and `--permanent` deletes them.
   - **Exception: `temp`.** Items in `/tmp` are always deleted. `/tmp` is tmpfs here and is wiped at boot anyway, and creating a `.Trash-$uid` there would be pointless.
   - **Exception: `trash`.** Emptying the trash is a permanent delete.
   - The preview says so for both: `permanently delete (temporary files)` and `permanently delete (emptying the trash)`.
   - When anything was trashed, the preview footer adds: `space is freed when the trash is emptied (or use --permanent)`.
2. **Safety scopes.** `safety::verify` gains a scope parameter: `Scope::Home` (today's behaviour), `Scope::UserTemp` and `Scope::HomeTrash`.
   - **`UserTemp`:** the path must be a direct child of in-system `/tmp`, owned by the user, a regular file or directory (no sockets, fifos or devices), and not a symlink.
   - **`HomeTrash`:** the path must be a direct child of `{data_home}/Trash/files` or `{data_home}/Trash/info`.
   - Protected-path and exclude checks still apply in every scope. M4 will add `Scope::System`.
3. **Flatpak cleanup is delegated to Flatpak.** `flatpak uninstall` has no dry-run, and working out unused runtimes and extensions ourselves would be fragile.
   - So `flatpak-unused` produces one item, an `Action::UserCommand` running the trusted-resolved `flatpak uninstall --user --unused --noninteractive -y`.
   - It has unknown size and is described as `remove runtimes no installed Flatpak app uses (decided by flatpak)`.
   - The item exists only when `flatpak list --user --runtime` lists at least one runtime.
   - System installations are M4.
4. **Only the home trash.** `trash` cleans only `{data_home}/Trash`; per-mount `.Trash-$uid` dirs are listed but never cleaned in M3.
   - Each item is one trashed entry, `files/<name>` paired with `info/<name>.trashinfo`, and both are deleted.
   - An orphaned `info` or `files` entry is still deleted.
5. **Which AppImages are "old".** AppImages are grouped by `normalize_name(split_appimage_name(..).0)`. Within a group, crumbs keeps:
   - the highest version;
   - any version that a desktop entry or autostart entry points at by `Exec`;
   - any version that is currently running.

   The rest are `appimage-old`. Versions are compared component by component (numeric where both components are numbers, otherwise lexical). A version that doesn't parse is never "old".
6. **Running apps.** A `browser-cache` or `cache` item whose owning app is running, or an `appimage-old` item that is running, is listed but not pre-selected. Its note reads `<app> is running`. Ownership comes from the cache dir name matching an inventory app's strong identifiers, using the M2 matcher's `score_name` High rule. Expose it as `pub(crate)` from `leftovers::score` if needed.
7. **Defaults.** With no category arguments, `clean` uses every enabled category: all user categories except `categories_off`, which defaults to `["dev-cache"]`.
   - Every item in the chosen categories is pre-selected, except those blocked by Decision 6.
   - `-i` opens a checklist over all items.
   - `--system` exits 2 with `system cleanup needs privileged support (coming in a later version)`.

## Global Constraints

- All M1 and M2 global constraints still apply. M2's are in `docs/superpowers/plans/2026-11-08-crumbs-m2-leftovers.md`, and its accepted behaviours are listed at the bottom of the M2 follow-ups note.
- One new dependency: `cargo add -p crumbs-core rayon`. Nothing else.
- New config keys, all optional: `temp_age_days = 7`, `log_age_days = 14`, `categories_off = ["dev-cache"]`.
  - Unknown category names in `categories_off` produce a `Config.warnings` entry and are otherwise ignored.
- Category names are exactly these, serialized in kebab-case: `cache`, `thumbnails`, `trash`, `temp`, `browser-cache`, `dev-cache`, `flatpak-unused`, `appimage-old`, `logs`.
- Every destructive item is verified with `safety::verify(env, cfg, scope, path, fp)` immediately before it acts. `UserCommand` items are re-checked by resolving the program in the trusted directories at execution time.
- `scan` is read-only and never writes anything, not even history.
- Performance:
  - `scan` (all enabled categories) must finish in under 10 s on the dev machine, whose `~/.cache` is 28 GB, with a warm cache, in a release build.
  - Sizes are computed in parallel across items with rayon.

## Review Focus

1. **Symlinks inside the cache.** If `~/.cache/foo` is a symlink to `/data/big`, the size counts only the link, and cleaning trashes only the link; `/data/big` is untouched. Pinned in Task 6.
2. **Live `/tmp` entries.** Never touch other users' files, sockets, fifos, or names with these prefixes: `.X`, `.ICE-unix`, `.font-unix`, `tmux-`, `ssh-`, `systemd-private-`, `pulse-`, `krb5cc`, `.s.PGSQL`. For a directory, the newest mtime anywhere inside it must also be older than the threshold. Pinned in Task 7.
3. **A running browser.** With `brave` running, `browser-cache` items for Brave are listed but not pre-selected, and their note says it is running. Pinned in Task 7.
4. **A launcher pointing at an older AppImage.** The newest version isn't what the user launches. `GutCheck-0.18` is kept when a launcher's Exec points at it. Pinned in Task 8.
5. **Emptying a trash with mismatched pairs.** `files/x` has no info, and `info/y.trashinfo` has no file. Both are listed and deleted. Trash dirs are validated, with the M2 rules, before anything is listed. A symlinked `Trash/files` gives a scanner error and no items. Pinned in Task 7.

---

## File Structure

```
crates/crumbs-core/src/bloat/mod.rs          # Category, BloatItem, ItemAction, Scanner, ScanReport, scan_all
crates/crumbs-core/src/bloat/cache.rs        # cache + thumbnails
crates/crumbs-core/src/bloat/browser.rs      # browser-cache
crates/crumbs-core/src/bloat/dev.rs          # dev-cache
crates/crumbs-core/src/bloat/logs.rs         # logs
crates/crumbs-core/src/bloat/temp.rs         # temp
crates/crumbs-core/src/bloat/trash.rs        # trash (emptying)
crates/crumbs-core/src/bloat/flatpak.rs      # flatpak-unused
crates/crumbs-core/src/bloat/appimage.rs     # appimage-old + version compare
crates/crumbs-core/src/bloat/tests.rs
crates/crumbs-cli/src/cmd/scan.rs
crates/crumbs-cli/src/cmd/clean.rs
```

Modify: `safety.rs`, `config.rs`, `size.rs`, `plan.rs`, `executor/mod.rs`, `history.rs`, `restore.rs`, `leftovers/{candidates,matcher}.rs`, `doctor.rs`, `lib.rs`, the CLI's `main.rs`, `cmd/{context,act,render,apps,uninstall}.rs` and `tests/cli.rs`, plus `docs/json-schema.md` and `README.md`.

---

### Task 1: Glob and exclude hardening (M3 entry criteria 1, 6)

**Files:** Modify `crates/crumbs-core/src/config.rs` and `crates/crumbs-core/src/safety.rs`, with tests in both.

**Interfaces / behaviour:**
- **`glob_match`** becomes a linear dynamic-programming matcher over path segments: no backtracking blow-up.
  - `**` matches zero or more whole segments, so `/a/.cache/**` matches `/a/.cache` itself.
  - `*` matches within one segment.
  - Existing semantics stay: a trailing `/` is trimmed, and a wildcard-free pattern matches the path and everything beneath it.
- **`pub fn could_match_below(pattern: &str, dir: &str) -> bool`:** true if some path strictly beneath `dir` could match `pattern`. That is, `dir`'s segments match a prefix of the pattern's segments, with `*` matching one segment and `**` matching any number.
- **Exclude in `is_allowed_user_target_by`:** a target is refused when `could_match_below(pattern, target)` for any exclude pattern. This replaces the literal-prefix rule from the M2 fix and covers it.
- **Normalising patterns:** `load_config` normalises `.` and `..` lexically in protect, exclude and appimage_dirs. A `..` that would climb above `/` drops the entry and adds a warning.

- [ ] **Step 1: Failing tests:**
  - `glob_double_star_matches_dir_itself`
  - `glob_many_double_stars_is_fast`: `/**/**/**/**/**/**/x` against a 40-segment path finishes in under 50 ms (assert with `Instant`).
  - `could_match_below_table`, covering:
    - `("~/.config/*/keep" expanded, "/h/.config/Foobar")` → true
    - `(…, "/h/.config")` → true
    - `(…, "/h/.cache/x")` → false
    - `("/h/a/**/z", "/h/a/b/c")` → true
    - `("/h/a/b", "/h/a")` → true
    - `("/h/a/b", "/h/a/b")` → false: strictly beneath, and equality is handled by `glob_match`.
  - `exclude_wildcard_shields_parent` (the M2 residual): `exclude = ["~/.config/*/keep"]`, so `~/.config/Foobar` is not an allowed target.
  - `config_normalises_dotdot`
- [ ] **Step 2: Run** `cargo test -p crumbs-core --features test-support config:: safety::` and expect FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** and expect PASS, plus the workspace. Commit `fix(core): segment glob matcher; exclude shields folders a pattern could match below`.

---

### Task 2: Safety scopes and bind-mount detection (Decision 2; entry criterion 2)

**Files:** Modify `safety.rs`, `size.rs` and `executor/mod.rs`, and update every caller of `verify`, `verify_now` and `is_allowed_user_target`.

**Interfaces:**
- `pub enum Scope { Home, UserTemp, HomeTrash }`, serde kebab-case, `Default = Home`.
- `pub fn is_allowed_target(env, cfg, scope: Scope, in_system: &Path) -> bool`. `is_allowed_user_target` becomes the `Home` case.
- `verify(env, cfg, scope, path, expected)` and `verify_now(env, cfg, scope, path)` gain the scope parameter.
  - The parent-chain symlink check walks from the scope's root: home, `/tmp`, or the Trash `files`/`info` dir.
  - `UserTemp` additionally requires `uid == env.uid` and a file type of regular file or directory.
- `PlannedAction` gains `pub scope: Scope`, with serde default `Home`, so M2 plans and history are unchanged. The executor passes it to `verify`.
- **Bind mounts:** `size::crosses_device` is renamed `crosses_mount`.
  - It compares the mount ID from `rustix::fs::statx(…, StatxFlags::MNT_ID)` (field `stx_mnt_id`), falling back to `st_dev` when `MNT_ID` isn't supported.
  - The device and mount function stays injectable.
  - The executor message stays `refusing: <path> contains another filesystem`.

- [ ] **Step 1: Failing tests:**
  - `user_temp_scope_rules`: a direct child file of `/tmp` owned by the user is allowed. Refused: a nested path, a symlink, a fifo, and anything under home.
  - `home_trash_scope_rules`: `Trash/files/x` and `Trash/info/x.trashinfo` are allowed. Refused: `Trash/files/x/y` and `Trash` itself.
  - `protect_applies_in_every_scope`
  - `crosses_mount_detects_same_dev_bind_mount`: an injected mount-id function returns a different ID for a nested dir while `st_dev` is the same.
  - The existing M2 tests stay green with `Scope::Home`.
- [ ] **Step 2: Run** and expect FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** the workspace and expect PASS. Commit `feat(core): safety scopes; mount-id aware delete guard`.

---

### Task 3: Pipeline gaps from M2 (entry criteria 3, 4, 5)

**Files:** Modify `restore.rs`, `executor/mod.rs`, `plan.rs` and `leftovers/{candidates,matcher}.rs`.

**Behaviour:**
- **Restore history per action:** `restore()` appends each Restore history entry right after its action, with a per-entry serialization failure isolated to that entry. This mirrors the M2 I5 fix in the executor.
- **Action dependencies:**
  - `PlannedAction` gains `pub depends_on: Option<usize>`, an index into the same plan, with serde default `None`.
  - `build_plan` sets every leftover action of a full AppImage uninstall to depend on the AppImage file action (index 0).
  - The executor skips an action whose dependency did not succeed. The result is `ok: false` with error `skipped: <display path of dependency> was not removed`, and it is recorded in history.
- **Autostart name matching:** autostart candidates are also scored by name, using the candidate file stem against the app's strong identifiers with the same scoring as Config/Data candidates. Example: appofholding's `~/.config/autostart/gutcheck.desktop` becomes a High leftover of AppImage `GutCheck`.

- [ ] **Step 1: Failing tests:**
  - `restore_appends_per_entry`
  - `leftovers_skipped_when_appimage_file_fails`: make the AppImage trash fail at execute, for example by occupying the trash name for the item with a `create_new` collision via a read-only `Trash/info`. Every other action is skipped and has a history entry, and `~/.config/X` still exists.
  - `autostart_name_matched`
  - `orphans_no_longer_list_named_autostart`
- [ ] **Step 2: Run** and expect FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** the workspace and expect PASS. Commit `fix(core): per-entry restore history; leftover actions depend on the AppImage file; autostart name matching`.

---

### Task 4: CLI consistency fixes (entry criterion 6)

**Files:** Modify `crumbs-cli/src/cmd/{context,apps,doctor,uninstall,render,restore}.rs`, `crumbs-core/src/{doctor.rs,leftovers/mod.rs,leftovers/candidates.rs}` and `tests/cli.rs`.

**Behaviour:**
- **`Context::load` failures under `--json`:** a config parse error or an inventory failure prints the envelope with `data: null` and an error `{source: "config"|"cli", message}`. The exit code is unchanged (1).
- **`doctor`:** `doctor(env, runner, cfg: &Config)` uses the loaded config, so `appimage_dirs` count. The CLI loads the config, and a parse error is handled as above.
- **`apps list --source`:** errors from sources that were filtered out don't print and don't affect the exit code.
- **`restore <path>`:** picks the newest *not yet restored* successful Trash entry for that path. If every selector matches nothing, it exits 2. A mix of matching and non-matching selectors exits 3 as before.
- **Dotfile kind:** a new `FindingKind::DotFile` (kebab-case `dot-file`) for regular-file dot entries in home. `DotDir` stays for directories.
- **`--keep-data`:** also keeps the app's own launchers and icons, using the same `is_own_launcher` rule as `--leftovers-only`, so they don't dangle.
- **Preview sanitising:** preview detail lines (edit lines) replace control characters (`char::is_control`, except tab) with `�`.

- [ ] **Step 1: Failing tests:**
  - `json_config_error_has_envelope`
  - `doctor_counts_configured_appimage_dirs`
  - `source_filter_scopes_errors`
  - `restore_path_skips_restored_entry`
  - `restore_unknown_selector_exit_2`
  - `dotfile_kind`
  - `keep_data_keeps_launchers`
  - `preview_sanitises_control_bytes`
- [ ] **Step 2: Run** and expect FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** the workspace and expect PASS. Update `docs/json-schema.md` for `dot-file`, the restore exit codes and the doctor change. Commit `fix(cli): json envelopes on load errors, doctor config, source scoping, restore selection`.

---

### Task 5: Parallel sizes and the bloat model

**Files:** Create `crumbs-core/src/bloat/mod.rs`. Modify `size.rs`, `config.rs` and `lib.rs`.

**Interfaces:**
- **Sizing:**
  - `size::disk_usage_many(env: &Env, paths: &[PathBuf]) -> Vec<DiskUsage>` computes each path's usage in parallel with rayon, preserving order. Each walk keeps M2 `disk_usage` semantics: no symlink following, no crossing devices, hard links counted once per walk.
  - `disk_usage` itself stays sequential.
- **Config:** `Config` gains `temp_age_days: u64` (default 7), `log_age_days: u64` (default 14) and `categories_off: Vec<Category>` (default `[DevCache]`). An unknown name becomes a warning.
- **Model:**
  - `pub enum Category { Cache, Thumbnails, Trash, Temp, BrowserCache, DevCache, FlatpakUnused, AppimageOld, Logs }`: kebab-case, with `as_str`, `FromStr`, `ALL` and `fn default_on(self) -> bool` (all true except `DevCache`).
  - `pub enum ItemAction { Trash, Delete, Command { program: String, args: Vec<String> } }`.
  - `pub struct BloatItem { pub category: Category, pub path: Option<PathBuf>, pub label: String, pub size: Option<u64>, pub rebuildable: bool, pub preselected: bool, pub note: Option<String>, pub action: ItemAction, pub scope: Scope, #[serde(skip)] pub fingerprint: Option<Fingerprint>, pub extra_paths: Vec<(PathBuf, Fingerprint)>, pub app_id: Option<String> }`
    - `extra_paths` are additional paths removed with the item (used by `trash` to pair `info/<name>.trashinfo`). Serialize them as a list of in-system paths only.
    - `path` is in-system and `None` only for `Command` items.
    - `label` is a display string for command items.
  - `pub struct ScanContext<'a> { pub env: &'a Env, pub cfg: &'a Config, pub inv: &'a Inventory, pub runner: &'a dyn CommandRunner, pub now: SystemTime }`
  - `pub trait Scanner: Send + Sync { fn category(&self) -> Category; fn scan(&self, cx: &ScanContext) -> Result<Vec<BloatItem>, CoreError>; }`
  - `pub struct CategoryReport { pub category: Category, pub items: Vec<BloatItem>, pub total: u64, pub unknown_sizes: usize, pub error: Option<String> }`
  - `pub struct ScanReport { pub categories: Vec<CategoryReport> }`
  - `pub fn scanners() -> Vec<Box<dyn Scanner>>`, in `Category::ALL` order. Until Tasks 6–9 add the real scanners, it returns only those already implemented.
  - `pub fn scan_all(cx: &ScanContext, which: &[Category]) -> ScanReport`:
    - runs the selected scanners in parallel (rayon);
    - fills missing sizes in one `disk_usage_many` pass;
    - sorts items by size (descending, `None` last), then path;
    - records a scanner `Err` as `error` without stopping the others.

- [ ] **Step 1: Failing tests:**
  - `disk_usage_many_matches_sequential`
  - `config_bloat_keys_and_unknown_category_warns`
  - `category_round_trip_and_defaults`
  - `scan_all_isolates_scanner_error`: a test scanner returning `Err` alongside one returning items.
- [ ] **Step 2: Run** and expect FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** and expect PASS. Commit `feat(core): bloat model, scanner registry, parallel sizes`.

---

### Task 6: Scanners `cache`, `thumbnails`, `logs`, `dev-cache`

**Files:** Create `bloat/cache.rs`, `bloat/logs.rs` and `bloat/dev.rs`, with tests in `bloat/tests.rs`.

**Behaviour.** Every item path passes `is_allowed_target(.., Scope::Home, ..)`, and its fingerprint is taken at scan time; failures are dropped.
- **`dev-cache` (`rebuildable: true`):**
  - cargo: `~/.cargo/registry/cache`, `~/.cargo/registry/src`, `~/.cargo/git/checkouts`
  - npm: `~/.npm/_cacache`
  - pnpm: `~/.local/share/pnpm/store`, `~/.cache/pnpm`
  - yarn: `~/.cache/yarn`
  - pip: `~/.cache/pip`
  - go: `~/.cache/go-build`, `~/go/pkg/mod/cache`
  - gradle: `~/.gradle/caches`
  - maven: `~/.m2/repository`
  - ccache: `~/.cache/ccache` and `~/.ccache`

  Only paths that exist are listed. The list is a `const` table of `(tool, path)`, and the label is the tool name.
- **`cache`:** one item per direct child of `cache_home`, excluding:
  - `thumbnails` and `crumbs`;
  - any child that is a browser-cache root (`mozilla`, `chromium`, `google-chrome`, `BraveSoftware`, `vivaldi`, `microsoft-edge`);
  - any child that is the first component of a dev-cache path.

  `rebuildable: true`. Owner detection and the running-app rule follow Decision 6. The label is the dir name.
- **`thumbnails`:** a single item, `cache_home/thumbnails`, when it exists.
- **`logs`:**
  - every `*.log` file under `state_home`, walked without following symlinks, whose mtime is older than `log_age_days`;
  - `~/.xsession-errors` and `~/.xsession-errors.old` older than `log_age_days`;
  - each file is one item.

- [ ] **Step 1: Failing tests:**
  - `cache_items_per_subdir_excluding_others`
  - `cache_symlink_counts_link_only` (Review Focus #1)
  - `cache_running_app_not_preselected`: fake `/proc/<pid>/exe` for an inventory app named `hauler`, with `~/.cache/hauler`.
  - `thumbnails_single_item`
  - `dev_cache_paths_listed_rebuildable`
  - `logs_age_filter`
  - `protected_or_excluded_never_listed`
- [ ] **Step 2: Run** and expect FAIL.
- [ ] **Step 3: Implement.** Register the scanners.
- [ ] **Step 4: Run** and expect PASS. Commit `feat(core): cache, thumbnails, logs, dev-cache scanners`.

---

### Task 7: Scanners `browser-cache`, `temp`, `trash`

**Files:** Create `bloat/browser.rs`, `bloat/temp.rs` and `bloat/trash.rs`, with tests.

**Behaviour:**
- **`browser-cache` (`rebuildable: true`):** one item per existing cache dir matching these globs, expanded per profile:
  - `~/.cache/mozilla/firefox/*/cache2`
  - `~/.cache/chromium/*/{Cache,Code Cache,GPUCache}`
  - `~/.cache/google-chrome/*/{Cache,Code Cache,GPUCache}`
  - `~/.cache/BraveSoftware/*/*/{Cache,Code Cache,GPUCache}` (covers Brave-Browser and Brave-Origin)
  - `~/.cache/vivaldi/*/{Cache,Code Cache,GPUCache}`
  - `~/.cache/microsoft-edge/*/{Cache,Code Cache,GPUCache}`
  - the same under `~/.var/app/{org.mozilla.firefox,org.chromium.Chromium,com.google.Chrome,com.brave.Browser,com.vivaldi.Vivaldi,com.microsoft.Edge}/cache/…` with the same relative patterns.

  The label is `<Browser> (<profile>)`. The browser counts as running when a process exe basename is in `{firefox, firefox-bin, chromium, chrome, brave, vivaldi-bin, msedge}`, or for a Flatpak, when `flatpak ps` lists the app ID (via the runner, errors ignored). A running browser makes its items not pre-selected, with the note `<Browser> is running` (Review Focus #3).
- **`temp`** (`Scope::UserTemp`, `ItemAction::Delete`): direct children of in-system `/tmp` that meet all of these:
  - owned by the user;
  - a regular file or directory (lstat);
  - not a symlink;
  - no name prefix from Review Focus #2;
  - mtime older than `temp_age_days`;
  - for a directory, the newest mtime anywhere inside (walked without following symlinks, without crossing devices, capped at 10 000 entries) is also older, and a directory over the cap is skipped.

  The note is `deleted permanently (temporary file)`.
- **`trash`** (`Scope::HomeTrash`, `ItemAction::Delete`):
  - validate `{data_home}/Trash`, `files` and `info` with the M2 trash-dir rules; on failure the scanner returns `Err` (Review Focus #5);
  - one item per `files/<name>`, with its matching `info/<name>.trashinfo` added as a paired second path;
  - orphaned info files and orphaned files are items too;
  - the label is the original path from `Path=` (percent-decoded, display path), or `<name> (no trash info)`;
  - per-mount `.Trash-$uid` dirs are not scanned in M3.

- [ ] **Step 1: Failing tests:**
  - `browser_profiles_found` (Firefox, Brave-Origin and Chrome fixtures)
  - `browser_running_not_preselected`
  - `flatpak_browser_cache_found`
  - `temp_rules` (Review Focus #2): run the filesystem cases for real: a socket, a fifo, prefixed names, a fresh file, an old dir containing a fresh file, an old file, and an old dir with only old contents. Only the last two become items.
  - `temp_owner_rule`: test `pub(crate) fn temp_candidate(meta_uid, file_type, …) -> bool` directly with another uid, because tests can't `chown`.
  - `trash_pairs_and_orphans` (Review Focus #5)
  - `trash_symlinked_files_dir_is_error`
- [ ] **Step 2: Run** and expect FAIL.
- [ ] **Step 3: Implement.** Register the scanners.
- [ ] **Step 4: Run** and expect PASS. Commit `feat(core): browser-cache, temp, trash scanners`.

---

### Task 8: Scanners `flatpak-unused` and `appimage-old`

**Files:** Create `bloat/flatpak.rs` and `bloat/appimage.rs`, with tests.

**Behaviour:**
- **`flatpak-unused`** (Decision 3):
  - Available when `resolve_trusted(env, "flatpak")` exists.
  - Run `flatpak list --user --runtime --columns=ref`. If it lists at least one runtime, return one item with:
    - `action: Command { program: "flatpak", args: ["uninstall","--user","--unused","--noninteractive","-y"] }`
    - `size: None`, `label: "unused Flatpak runtimes (user)"`, `note: "decided by flatpak"`
    - `rebuildable: false`, `preselected: true`
  - A non-zero exit becomes `Err`.
- **`appimage-old`** (Decision 5):
  - `pub fn compare_versions(a: &str, b: &str) -> Option<Ordering>`: split on `.`, `-` and `_`; compare numerically where both components are digits, otherwise lexically; a missing component counts as less; returns `None` if either version is empty.
  - Group the inventory's AppImage apps by normalized base name.
  - Keep the maximum version, any AppImage whose in-system location equals the resolved `Exec` program of any desktop or autostart entry (via `discover_desktop_entries` plus `config_home/autostart`), and any running one (`running_processes`).
  - The rest become `Trash` items: label = file name, `app_id` set, `Scope::Home`.

- [ ] **Step 1: Failing tests:**
  - `flatpak_unused_item_when_runtimes`
  - `flatpak_unused_none_when_no_runtimes`
  - `flatpak_missing_is_unavailable_not_error`
  - `compare_versions_table`:
    - `0.19.0 > 0.18.9`
    - `1.10 > 1.9`
    - `2.0-beta` vs `2.0` → `2.0` is greater, because a missing component counts as less
    - `""` → `None`
  - `appimage_old_keeps_newest`
  - `appimage_old_keeps_launcher_target` (Review Focus #4)
  - `appimage_old_keeps_running`
  - `unparseable_version_never_old`
- [ ] **Step 2: Run** and expect FAIL.
- [ ] **Step 3: Implement.** Register the scanners.
- [ ] **Step 4: Run** and expect PASS. Commit `feat(core): flatpak-unused and appimage-old scanners`.

---

### Task 9: Clean plans and the `UserCommand` action

**Files:** Modify `plan.rs`, `executor/mod.rs`, `history.rs` and `restore.rs`, with tests.

**Interfaces:**
- **New action:** `Action::UserCommand { program: String, args: Vec<String> }` (tag `user-command`).
  - The executor resolves `program` with `resolve_trusted` at execution time. If it is missing, the result is `ok: false` with `<program> is not installed`.
  - It runs through `SystemRunner`. A non-zero exit gives `ok: false` with the stderr's last line.
  - There is no `safety::verify` for commands, since they have no path.
  - History action `command`: `path` = the program's in-system path; the history entry gains `argv: Option<Vec<String>>` (serde default).
  - Not restorable: restore reports `commands cannot be undone`.
- **`pub fn build_clean_plan(env: &Env, items: &[BloatItem], opts: &PlanOptions) -> Result<Plan, CoreError>`:**
  - `ItemAction::Trash` gives `Action::Trash`, or `Delete` with `--permanent`.
  - `ItemAction::Delete` gives `Delete` regardless.
  - `Command` gives `UserCommand`.
  - Each action carries the item's scope and fingerprint. Trash items with `extra_paths` produce one Delete per path.
  - `app_id` is `None`, and the plan notes list Decision 1's wording when relevant.
- **Delete guard:** the executor's Delete uses `crosses_mount` (Task 2) for every scope.

- [ ] **Step 1: Failing tests:**
  - `clean_plan_maps_actions`
  - `clean_plan_trash_pairs`
  - `permanent_turns_trash_into_delete`
  - `user_command_runs_trusted_program`: a fixture `/usr/bin/flatpak` script records its args.
  - `user_command_missing_program`
  - `restore_reports_commands_not_undoable`
  - `temp_delete_verified_in_user_temp_scope`
- [ ] **Step 2: Run** and expect FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** the workspace and expect PASS. Commit `feat(core): clean plans and user commands`.

---

### Task 10: CLI `scan`

**Files:** Create `crumbs-cli/src/cmd/scan.rs`. Modify `main.rs`, `cmd/context.rs` (add `ScanContext` construction; the leftover index isn't needed for `scan`, so make it lazy or split the loader) and `tests/cli.rs`.

**Behaviour:**
- **Command:** `crumbs scan [--category <c>]... [--user | --system] [--details] [--json]`.
- **`--system`:** exits 2 with the Decision 7 message.
- **Human output:**
  - A table `CATEGORY  ITEMS  SIZE  NOTE`, one row per scanned category in `Category::ALL` order.
    - SIZE is the total, followed by `+` when some sizes are unknown.
    - NOTE is `off (enable with --category or config)` for an off category that wasn't explicitly requested, `rebuildable` where it applies, or the scanner error.
  - A total line: `reclaimable: <size> across N items`.
  - `--details` also lists the items under each category: `SIZE  PATH/LABEL  NOTE`.
- **Off categories:** they are listed but not scanned unless named with `--category`.
- **JSON:** the `ScanReport` (fingerprints skipped) inside the envelope, `command: "scan"`.
- **Exit code:** 3 when any scanner failed.

- [ ] **Step 1: Failing tests:**
  - `scan_table_snapshot`
  - `scan_json_shape`
  - `scan_category_filter`
  - `scan_off_category_listed_not_scanned`
  - `scan_scanner_error_exit_3`
  - `scan_system_exit_2`
  - `scan_writes_nothing`: the fixture tree is byte-identical before and after.
- [ ] **Step 2: Run** and expect FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** and expect PASS. Run a real-system read-only check, `cargo run -q --release -p crumbs-cli -- scan`, timed with `time`, and paste the output and timing into the report. It must be under 10 s. Commit `feat(cli): scan`.

---

### Task 11: CLI `clean`

**Files:** Create `crumbs-cli/src/cmd/clean.rs`. Modify `main.rs`, `cmd/act.rs` and `cmd/render.rs` (render `user-command` rows as `run  —  flatpak uninstall --user --unused …`, plus the Decision 1 wording), and `tests/cli.rs`.

**Behaviour:**
- **Command:** `crumbs clean [<category>...] [--user | --system] [-i] [--dry-run] [--yes] [--permanent] [--force] [--json]`.
- **Selection (Decision 7):**
  1. Scan the chosen categories.
  2. Build items from the pre-selected ones, or with `-i`, from a checklist over all items whose defaults are the pre-selected ones.
  3. Run `build_clean_plan`.
  4. Then follow the shared preview → confirm → execute → report flow from `cmd/act.rs`, with the non-interactive rule, `--json` requiring `--yes`/`--dry-run`, and envelopes on failures.
- **Refusals and order:** a scanner error refuses execution without `--force`, while `--dry-run` still previews, the same rule as uninstall. A running-app check isn't needed here, since running items are already excluded by Decision 6.
- **Report:** the M2 report, plus `freed <size>` counting only Delete and permanent actions. Trashed sizes are reported as `moved <size> to the trash`. The undo hint appears when anything was trashed.
- **No category arguments and nothing pre-selected:** prints `nothing to clean` and exits 0.

- [ ] **Step 1: Failing tests** (everything inside the `CRUMBS_ROOT` fixture):
  - `clean_cache_trashes_and_logs`
  - `clean_temp_deletes_permanently_with_wording`
  - `clean_trash_empties_pairs`
  - `clean_flatpak_runs_command`: the fake flatpak script records its args.
  - `clean_dry_run_changes_nothing`
  - `clean_non_interactive_without_yes_exit_2`
  - `clean_json_requires_yes_or_dry_run`
  - `clean_scanner_error_refuses_without_force`
  - `clean_restore_round_trip`: clean the cache, then restore the batch.
  - `clean_system_exit_2`
  - `clean_off_category_needs_naming`: `dev-cache` isn't cleaned unless it is named.
- [ ] **Step 2: Run** and expect FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** `cargo test --workspace --all-features` and expect PASS. Real-system check, read-only: `cargo run -q -p crumbs-cli -- clean --dry-run`. Paste the output. **Never run `clean` without `--dry-run` on the real system.**
- [ ] **Step 5: Commit** `feat(cli): clean`.

---

### Task 12: Docs

**Files:** Modify `docs/json-schema.md`, `README.md` and the spec's milestone list.

- `json-schema.md`:
  - add the `scan` and `clean` data shapes;
  - document `BloatItem` (fingerprint skipped, `extra_paths` paths only), `Category` names, the `scope` field on planned actions, the `user-command` action, the `command` history action with `argv`, `depends_on`, and the `skipped:` error;
  - examples come from the debug binary against a fixture, as in M2.
- README:
  - status M3;
  - `scan` and `clean` usage;
  - the Decision 1 wording about trash versus freed space;
  - the `categories_off`, `temp_age_days` and `log_age_days` config keys.
- Spec §13: mark milestones 1–3 as done (append `— done`).

- [ ] **Step 1: Write the docs** from real output.
- [ ] **Step 2: Cross-check** every documented field against the serde structs, as in M2 Task 13.
- [ ] **Step 3: Commit** `docs: scan and clean`.
