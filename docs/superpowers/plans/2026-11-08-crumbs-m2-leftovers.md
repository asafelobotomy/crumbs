# crumBS Milestone 2 (Leftovers & User-Level Uninstall) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `crumbs` finds each app's leftover files with a confidence level, moves the user's selection to the freedesktop trash after a preview, logs every action, and can restore it. It also uninstalls AppImages and lists orphaned config/cache/data folders.

**Architecture:** New core modules follow the M1 pattern (everything through `Env`, in-system paths in results, no terminal I/O in core). The flow is `matcher`/`orphans` → `Vec<Finding>` → `planner::build_plan` → `executor::execute` (`safety` verification → `trash`/delete/edit → `history`). The CLI adds `uninstall`, `orphans`, `history` and `restore`, and `apps show` gains leftovers.

**Tech Stack:** M1 stack plus `toml` (rules + config), `jiff` (timestamps) and `dialoguer` (CLI prompts and checklists).

**Spec:** `docs/superpowers/specs/2026-11-08-crumbs-design.md`: sections 4, 6, 8, 9 and 10, milestone 2 of section 13. Also the M2 entry criteria in `docs/superpowers/notes/2026-11-08-m1-review-followups.md`.

## Decisions this plan makes (the spec leaves them open)

1. **App IDs become unique.**
   - AppImage: `appimage:<display path>`, where the display path is `~/…` under home and absolute otherwise. Example: `appimage:~/Applications/GutCheck-0.19.0-x86_64.AppImage`.
   - Flatpak: `flatpak:user/<id>` or `flatpak:system/<id>`. Other sources are unchanged.
2. **M2 removes only user-level things.**
   - Full uninstall is supported only for AppImages: trash the file, its linked desktop entries and its leftovers.
   - pacman, Flatpak and unmanaged apps require `--leftovers-only`. Without it, crumbs exits 2 with `removing <source> apps needs privileged support (coming in a later version); use --leftovers-only`. Package removal is M4.
3. **System leftovers (`/etc`, `/opt`) are not searched in M2.** They need directory ownership and privilege, so they are M4.
4. **Size calculation now.** A minimal sequential `size` module is added now, because leftovers must show sizes. M3 parallelises it.
5. **Own trash implementation.** crumbs implements the freedesktop trash spec itself instead of using a crate, because it must go through `Env` (fake roots in tests). It only creates `$topdir/.Trash-$uid`, never the shared `$topdir/.Trash/$uid`.
6. **Config keys.** `config.toml` gets only the keys M2 uses: `protect`, `exclude` and `appimage_dirs`. M3 adds the rest.
7. **Exact match for destructive commands.** `uninstall` only accepts `find_app` matches from steps 1-5 (exact). A step-6 (normalized) match exits 2 with `did you mean '<id>'?`. If inventory has source errors, `uninstall` refuses (exit 1) unless `--force`.
8. **`--root` is debug-only.** `--root`/`CRUMBS_ROOT` exists only in debug builds (`cfg(debug_assertions)`). Release binaries cannot use a fake root.

## Global Constraints

- All M1 global constraints still apply (plan `2026-11-08-crumbs-m1-foundation.md`): in-system paths in results, `env.sys` for every system path, no `unwrap`/`expect` in library code, clippy `-D warnings`, fmt, the JSON envelope `schema_version: 1`, exit codes 0/1/2/3/4/5, and IEC sizes.
- New dependencies, added with `cargo add`:
  - core: `toml`, `jiff`
  - cli: `dialoguer`

  Nothing else.
- Nothing in core deletes, renames or writes outside the paths given by a `Plan`, the trash directories, `{state_home}/crumbs/` and backup files next to edited files.
- Trash is the default for every user file. `Delete` only happens with `--permanent`.
- Every action is re-verified with `safety::verify` immediately before it runs.
- History file: `{state_home}/crumbs/history.jsonl`, one JSON object per line, append-only.
- Interactive prompts happen only when stdin and stdout are TTYs. A destructive command that is not interactive and has no `--yes` exits 2 with `refusing to act without --yes in a non-interactive session`.
- Matching never uses an identifier shorter than 3 characters, or one in the generic stoplist: `app apps bin lib lib64 share data cache config local icons applications autostart systemd fonts themes java python node npm git go user default common settings`. Identifiers come only from rules in those cases.

## Review Focus

1. **Path swapped between scan and action.** A finding's directory is replaced by a symlink to `~/Documents` after the scan. The action must be refused because `(dev, ino)` no longer matches, and nothing under `~/Documents` is touched. Pinned in Task 4.
2. **A folder belonging to two installed apps.** `~/.config/chromium` matches both `chromium` and a `chromium-widevine` package. It is marked `shared`, downgraded one level, and not pre-selected. Pinned in Task 7.
3. **Trash name collisions.** Trashing two different `config` dirs means the second gets `config.2` in `Trash/files`, both `.trashinfo` files are valid, and both are restorable. Pinned in Task 9.
4. **Restore onto an occupied path.** The original path has been recreated since the item was trashed. Restore refuses that item and leaves it in the trash, and the other items in the same request still restore. Pinned in Task 10.
5. **Short or generic app names.** An app named `go` with exec name `code` must not match `~/.config/go`, and `~/.cache/code-insiders` must not match through substring rules. Identifiers under 3 characters and stoplisted ones are ignored, and Low matches are hidden unless `--show-low`. Pinned in Task 7.

---

## File Structure

```
crates/crumbs-core/src/config.rs          # Config { protect, exclude, appimage_dirs }
crates/crumbs-core/src/safety.rs          # protected paths, allowed roots, fingerprint verify
crates/crumbs-core/src/size.rs            # disk_usage()
crates/crumbs-core/src/rules.rs           # Rule loading (embedded + user rules.d)
crates/crumbs-core/src/leftovers/mod.rs   # Finding, FindingKind, Confidence
crates/crumbs-core/src/leftovers/matcher.rs   # find_leftovers()
crates/crumbs-core/src/leftovers/orphans.rs   # find_orphans()
crates/crumbs-core/src/trash.rs           # freedesktop trash: move_to_trash, restore_from_trash
crates/crumbs-core/src/history.rs         # HistoryEntry, append, read
crates/crumbs-core/src/plan.rs            # Plan, Action, build_plan
crates/crumbs-core/src/executor.rs        # execute(), running-app check, ExecReport
crates/crumbs-core/src/restore.rs         # restore()
data/rules/apps.toml                      # curated rules (embedded with include_str!)
crates/crumbs-cli/src/prompt.rs           # confirm / checklist helpers (dialoguer)
crates/crumbs-cli/src/cmd/{uninstall.rs,orphans.rs,history.rs,restore.rs}
docs/json-schema.md
```

Modify: `sources/appimage.rs`, `sources/flatpak.rs`, `sources/unmanaged.rs`, `inventory.rs`, `env.rs`, `lib.rs`, `testing.rs`, `crumbs-cli/src/{main.rs,output.rs,cmd/apps.rs}`, `crumbs-cli/tests/cli.rs`, `README.md`.

---

### Task 1: Unique IDs and AppImage detection hardening (M1 follow-ups)

**Files:** Modify `sources/appimage.rs`, `sources/flatpak.rs`, `sources/unmanaged.rs` and their tests, plus `crumbs-cli/tests/cli.rs` and its snapshot.

**Interfaces:**
- Produces:
  - `pub fn display_path(env: &Env, in_system: &Path) -> String` in `env.rs`. Under the in-system home it returns `~/<rest>`, otherwise the absolute path.
  - AppImage IDs are `appimage:{display_path}`.
  - Flatpak IDs are `flatpak:user/<id>` or `flatpak:system/<id>`. `name` stays the bare `<id>`.
  - `is_appimage(bytes)` also accepts Type-1, which is `AI\x01` at bytes 8..11.
- Behaviour change in unmanaged: if the resolved program cannot be canonicalized, or its first 16 bytes cannot be read, the entry is **skipped** (fail closed). Today it falls through to unmanaged.

- [ ] **Step 1: Failing tests:**
  - `appimage_ids_include_path`: the same file name in `~/Applications` and `~/Downloads` gives two distinct IDs, `appimage:~/Applications/X-1.0-x86_64.AppImage` and `appimage:~/Downloads/X-1.0-x86_64.AppImage`.
  - `flatpak_ids_include_installation`: rows for `org.x.Y` in both `user` and `system` give `flatpak:user/org.x.Y` and `flatpak:system/org.x.Y`.
  - `type1_magic_detected`
  - `unreadable_program_not_unmanaged`: a program file with mode `0o000`. Skip the test when running as root, since root can read everything.
  - `display_path_home_and_absolute`
- [ ] **Step 2: Run** `cargo test -p crumbs-core --features test-support` and expect FAIL.
- [ ] **Step 3: Implement**, then update the CLI tests and snapshot for the new IDs. `find_app` already matches `name` and `rdns_id`, so `apps show org.x.Y` with both installations present becomes Ambiguous.
- [ ] **Step 4: Run** `cargo test --workspace --all-features` and expect PASS. Commit `feat(core): unique app ids; harden appimage detection`.

---

### Task 2: CLI hardening (M1 follow-ups)

**Files:** Modify `crumbs-cli/src/main.rs`, `crumbs-cli/src/cmd/apps.rs`, `crumbs-cli/src/output.rs`, `crumbs-core/src/env.rs` and `crumbs-cli/tests/cli.rs`.

**Interfaces / behaviour:**
- **Debug-only `--root`:** the `root` field and its use exist only under `#[cfg(debug_assertions)]`. Tests run debug binaries, so they keep working.
- **Stricter fake root:** `Env::fake` also returns `UnsafeRoot` when the canonical temp dir is `/` or has fewer than 2 path components (`/tmp` is allowed, `/` is not).
- **EPIPE:** a `BrokenPipe` write error anywhere in output ends the process with exit 0 and no message. Implement this in `main`, by checking the error chain for `io::ErrorKind::BrokenPipe`.
- **`apps show` exit code:** it exits 3 when the inventory has source errors and the app was found, the same rule as `apps list`. Move `partial_failure*` from `main.rs` to `output.rs`.
- **Installation in human output:** printed via serde names (`user`/`system`), not `Debug`.

- [ ] **Step 1: Failing tests:**
  - `env::fake_rejects_root_temp_dir`: test a helper `pub(crate) fn check_fake_root(root: &Path, temp: &Path) -> Result<PathBuf, CoreError>` with `temp = /`, which `Env::fake` uses. Never mutate the process environment in tests.
  - `cli::apps_list_pipe_closed_exit_0`: spawn the binary with stdout piped, close the read end immediately, and assert exit 0 with stderr empty.
  - `cli::apps_show_partial_failure_exit_3`
- [ ] **Step 2: Run** and expect FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** `cargo test --workspace --all-features` and `cargo build --release -p crumbs-cli`, then confirm that `target/release/crumbs --root /tmp doctor` exits 2 with an `unexpected argument` error. Commit `fix(cli): debug-only root override, EPIPE, consistent exit codes`.

---

### Task 3: Config loader

**Files:** Create `crumbs-core/src/config.rs`, and wire `appimage_dirs` into `sources/appimage.rs`.

**Interfaces:**
- `pub struct Config { pub protect: Vec<PathBuf>, pub exclude: Vec<String>, pub appimage_dirs: Vec<PathBuf> }` with `Default` (all empty) and `Deserialize`. Unknown keys are ignored, so M3 keys don't break older binaries.
- `pub fn load_config(env: &Env) -> Result<Config, CoreError>` reads `{config_home}/crumbs/config.toml`. A missing file gives `Ok(Config::default())`. A parse error gives `CoreError::Parse { what: "config.toml", .. }`. `~` in paths is expanded to the in-system home. Stored paths are in-system.
- `exclude` entries are glob patterns matched against in-system paths. Support only `*` (one segment), `**` (any depth) and literal paths, through a small `pub fn glob_match(pattern: &str, path: &str) -> bool`. Do not add a glob crate.
- `SourceContext` gets `pub config: &'a Config`. `discover` becomes `discover(env, runner, opts, cfg: &Config)`. Update every caller (`doctor`, the CLI, tests); they pass `&Config::default()` until the CLI loads the real config in Task 11. The AppImage source scans `cfg.appimage_dirs` in addition to its defaults.

- [ ] **Step 1: Failing tests:**
  - `missing_config_is_default`
  - `parses_keys_and_expands_tilde`
  - `unknown_keys_ignored`
  - `bad_toml_is_parse_error`
  - `glob_match`, a table test:
    - `~/.cache/**` style patterns against an in-system path;
    - `*` does not cross `/`;
    - a literal path matches exactly.
  - `appimage_dirs_scanned`
- [ ] **Step 2: Run** and expect FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** and expect PASS. Commit `feat(core): config loader`.

---

### Task 4: Safety module

**Files:** Create `crumbs-core/src/safety.rs`.

**Interfaces:**
- `pub enum Protection { Exact, Subtree }`
- `pub fn protected_paths(env: &Env, cfg: &Config) -> Vec<(PathBuf, Protection)>` returns in-system paths, exactly the spec §8 lists plus `cfg.protect` as Subtree.
- `pub fn is_protected(env: &Env, cfg: &Config, in_system: &Path) -> bool`:
  - **Exact:** `path == p`.
  - **Subtree:** `path == p` or `path` starts with `p`.
  - **Ancestors:** a path that is an **ancestor** of any protected path is also protected. Trashing `~/.local` would take `~/.local/share/keyrings` with it.
- `pub fn is_allowed_user_target(env: &Env, cfg: &Config, in_system: &Path) -> bool`: true when the path is strictly under the in-system home, is not protected, and does not match `cfg.exclude`.
- `pub struct Fingerprint { pub dev: u64, pub ino: u64, pub is_symlink: bool }`, Serialize and Deserialize.
- `pub fn fingerprint(env: &Env, in_system: &Path) -> Result<Fingerprint, CoreError>` uses `symlink_metadata` on the root-joined path.
- `pub fn verify(env: &Env, cfg: &Config, in_system: &Path, expected: &Fingerprint) -> Result<(), SafetyError>`:
  - Fails with `SafetyError::{Protected, OutsideAllowedRoots, Changed, Missing}`.
  - Every parent directory between home and the path must not be a symlink (`symlink_metadata` on each). Otherwise it fails with `Changed`.
- `SafetyError` is a `thiserror` enum with Display messages such as `refusing: <path> is protected`.

- [ ] **Step 1: Failing tests:**
  - `home_itself_protected_exact_child_allowed`: `~` is protected, `~/.config/foo` is allowed, and `~/.config` itself is protected.
  - `ssh_subtree_protected`: `~/.ssh/known_hosts` is protected.
  - `ancestor_of_protected_is_protected`: `~/.local` and `~/.local/share` are protected because keyrings sits beneath them.
  - `user_protect_added`
  - `exclude_glob_blocks_target`
  - `outside_home_not_allowed`: `/opt/foo` and `/tmp/x` are not allowed.
  - `verify_detects_swap` (Review Focus #1): fingerprint `~/.config/foo` (a dir), replace it with a symlink to `~/Documents`, and `verify` gives `Err(Changed)`.
  - `verify_detects_symlinked_parent`: `~/.config` replaced by a symlink gives `Err(Changed)`.
  - `verify_missing`
- [ ] **Step 2: Run** `cargo test -p crumbs-core --features test-support safety::` and expect FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** and expect PASS. Commit `feat(core): safety checks`.

---

### Task 5: Size module

**Files:** Create `crumbs-core/src/size.rs`.

**Interfaces:**
- `pub struct DiskUsage { pub bytes: u64, pub files: u64, pub errors: u64 }` (Serialize).
- `pub fn disk_usage(env: &Env, in_system: &Path) -> DiskUsage` measures allocated size (`st_blocks * 512`).
  - It does not follow symlinks, so a symlink counts its own blocks.
  - It does not cross devices: entries whose `st_dev` differs from the start path's are skipped.
  - It counts each `(dev, ino)` only once.
  - Unreadable entries increment `errors` and the walk continues.
  - It is sequential and iterative (an explicit stack, no recursion).

- [ ] **Step 1: Failing tests:**
  - `sums_files_in_tree`: the result is at least the sum of the file lengths, and `files == 3`.
  - `hardlink_counted_once`
  - `symlink_not_followed`: a symlink to a big file adds less than the big file's size.
  - `unreadable_dir_counts_error`: a dir with mode `0o000`; skip when running as root.
  - `missing_path_is_zero_with_error`
- [ ] **Step 2: Run** and expect FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** and expect PASS. Commit `feat(core): disk usage`.

---

### Task 6: Rules loader and curated rules

**Files:** Create `crumbs-core/src/rules.rs` and `data/rules/apps.toml`.

**Interfaces:**
- TOML format (spec §4):

  ```toml
  [[app]]
  match = { package = ["..."], flatpak = ["..."], appimage = ["..."], desktop = ["..."] }
  paths = ["~/.mozilla", "$XDG_CACHE_HOME/mozilla"]
  ```

  `appimage` values are compared with `normalize_name` against the AppImage `name`. `desktop` values are desktop IDs without `.desktop`.
- `pub struct Rule { pub packages: Vec<String>, pub flatpaks: Vec<String>, pub appimages: Vec<String>, pub desktops: Vec<String>, pub paths: Vec<String> /* raw */, pub origin: RuleOrigin }`
- `pub enum RuleOrigin { Builtin, User(PathBuf) }`
- `pub fn load_rules(env: &Env) -> (Vec<Rule>, Vec<CoreError>)`:
  - Builtin rules come from `include_str!("../../../data/rules/apps.toml")`. A parse error in builtin rules fails a unit test, never runtime.
  - User rules come from `{config_home}/crumbs/rules.d/*.toml`, sorted by file name. A bad user file is returned as an error and skipped.
- `pub fn rule_matches(rule: &Rule, app: &App) -> bool` compares `package` with `app.name` for pacman/unmanaged, `flatpak` with the rdns ID, `appimage` by normalized name, and `desktop` against `app.desktop_ids`.
- `pub fn rules_for<'a>(rules: &'a [Rule], app: &App) -> Vec<&'a Rule>`: if any **user** rule matches, only the matching user rules are returned (user precedence); otherwise the matching builtin rules.
- `pub fn expand_rule_path(env: &Env, raw: &str) -> Option<PathBuf>` expands `~`, `$HOME`, `$XDG_CONFIG_HOME`, `$XDG_CACHE_HOME`, `$XDG_DATA_HOME` and `$XDG_STATE_HOME` to in-system paths. It returns `None` for anything not under home after expansion, or containing `..`.
- `apps.toml` content covers at least these apps:
  - Firefox (pacman `firefox`, Flatpak `org.mozilla.firefox`): `~/.mozilla/firefox`, `$XDG_CACHE_HOME/mozilla/firefox`
  - Thunderbird: `~/.thunderbird`, `$XDG_CACHE_HOME/thunderbird`
  - Chromium: `$XDG_CONFIG_HOME/chromium`, `$XDG_CACHE_HOME/chromium`
  - Google Chrome (`google-chrome`): `$XDG_CONFIG_HOME/google-chrome`, `$XDG_CACHE_HOME/google-chrome`
  - Brave (`brave-bin`, `brave-browser`, `com.brave.Browser`): `$XDG_CONFIG_HOME/BraveSoftware`, `$XDG_CACHE_HOME/BraveSoftware`
  - VS Code (`visual-studio-code-bin`, `code`, `com.visualstudio.code`): `$XDG_CONFIG_HOME/Code`, `~/.vscode`
  - VSCodium (`vscodium`, `vscodium-bin`, `com.vscodium.codium`): `$XDG_CONFIG_HOME/VSCodium`, `~/.vscode-oss`
  - Cursor (`cursor-bin`, AppImage `cursor`): `$XDG_CONFIG_HOME/Cursor`, `~/.cursor`
  - Discord (`discord`, `com.discordapp.Discord`): `$XDG_CONFIG_HOME/discord`
  - Spotify (`spotify`, `com.spotify.Client`): `$XDG_CONFIG_HOME/spotify`, `$XDG_CACHE_HOME/spotify`
  - Steam (`steam`, `com.valvesoftware.Steam`): `~/.steam`, `$XDG_DATA_HOME/Steam`
  - OBS (`obs-studio`, `com.obsproject.Studio`): `$XDG_CONFIG_HOME/obs-studio`
  - GIMP (`gimp`, `org.gimp.GIMP`): `$XDG_CONFIG_HOME/GIMP`, `$XDG_CACHE_HOME/gimp`
  - Inkscape (`inkscape`, `org.inkscape.Inkscape`): `$XDG_CONFIG_HOME/inkscape`, `$XDG_CACHE_HOME/inkscape`
  - LibreOffice (`libreoffice-fresh`, `libreoffice-still`, `org.libreoffice.LibreOffice`): `$XDG_CONFIG_HOME/libreoffice`
  - Blender (`blender`, `org.blender.Blender`): `$XDG_CONFIG_HOME/blender`
  - Godot (`godot`, `org.godotengine.Godot`): `$XDG_CONFIG_HOME/godot`, `$XDG_DATA_HOME/godot`, `$XDG_CACHE_HOME/godot`
  - Telegram (`telegram-desktop`, `org.telegram.desktop`): `$XDG_DATA_HOME/TelegramDesktop`
  - Signal (`signal-desktop`, `org.signal.Signal`): `$XDG_CONFIG_HOME/Signal`
  - Slack (`slack-desktop`, `com.slack.Slack`): `$XDG_CONFIG_HOME/Slack`
  - Zoom (`zoom`, `us.zoom.Zoom`): `~/.zoom`, `$XDG_CONFIG_HOME/zoomus.conf`
  - Android Studio (`android-studio`): `~/.android`, `$XDG_CONFIG_HOME/Google/AndroidStudio*`, `$XDG_CACHE_HOME/Google/AndroidStudio*`. A trailing `*` in a rule path means every sibling that starts with the prefix; this is the only glob rules support.
  - Mozilla parent dirs (`~/.mozilla`, `$XDG_CACHE_HOME/mozilla`) are deliberately **not** listed, because they are shared by Firefox and Thunderbird.

- [ ] **Step 1: Failing tests:**
  - `builtin_rules_parse`: no errors, and at least 20 rules.
  - `user_rules_override_builtin`
  - `bad_user_rule_reported_and_skipped`
  - `expand_rule_path_tokens`
  - `expand_rejects_outside_home_and_dotdot`
  - `rule_matches_by_source`: a Flatpak app matches by rdns, an AppImage by normalized name.
  - `prefix_glob_expands`: with `AndroidStudio2025.1` and `AndroidStudio2025.2` present, both are returned. Add `pub fn resolve_rule_paths(env: &Env, rule: &Rule) -> Vec<PathBuf>`, which returns only paths that exist.
- [ ] **Step 2: Run** and expect FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** and expect PASS. Commit `feat(core): rules loader and curated rules`.

---

### Task 7: Leftover model and matcher

**Files:** Create `crumbs-core/src/leftovers/mod.rs` and `crumbs-core/src/leftovers/matcher.rs`.

**Interfaces:**
- `pub enum Confidence { Low, Medium, High }` (Ord in that order, Serialize lowercase).
- `pub enum FindingKind { Config, Cache, Data, State, DotDir, FlatpakData, SnapData, Autostart, DesktopEntry, Icon, SystemdUnit, MimeAssoc }` (kebab-case).
- `pub struct Finding { pub path: PathBuf /* in-system */, pub kind: FindingKind, pub confidence: Confidence, pub shared: bool, pub reason: String, pub size: DiskUsage, pub preselected: bool, #[serde(skip)] pub fingerprint: Fingerprint, pub mime_lines: Vec<String> /* MimeAssoc only */ }`
- `pub struct LeftoverIndex`, built once per inventory:
  - `pub fn build(env: &Env, cfg: &Config, rules: &[Rule], inv: &Inventory) -> LeftoverIndex`
  - It enumerates candidates once and records, for each candidate path, which app IDs match it and how strongly.
- `pub fn find_leftovers(idx: &LeftoverIndex, app: &App, show_low: bool) -> Vec<Finding>`. Results are sorted by confidence (descending), then path.
- **Candidates:**
  - **Dirs and files directly inside the XDG dirs:** each direct child of `config_home`, `cache_home`, `data_home` and `state_home`, giving kinds Config, Cache, Data and State. Skip `data_home/{applications,icons,Trash,flatpak,mime}` and `cache_home/thumbnails` as candidates themselves.
  - **Home entries:** direct children of home whose name starts with `.`, excluding the XDG dirs, `.local` and `.var` (kind DotDir).
  - **Flatpak data:** `~/.var/app/<id>` (FlatpakData), High for the Flatpak app with that ID and never matched otherwise.
  - **Snap data:** `~/snap/<name>` (SnapData).
  - **Launchers:** `config_home/autostart/*.desktop` (Autostart) and `data_home/applications/*.desktop` that are not already claimed by an AppImage app (DesktopEntry). These are matched by desktop ID, and by `exec_program` equal to the app's location or exec names.
  - **Icons:** files under `data_home/icons/**` whose stem equals one of the app's desktop-ID stems or its rdns ID (Icon), always High.
  - **User services:** `config_home/systemd/user/*.service` whose `ExecStart=` program basename is in the app's `exec_names` or equals its location (SystemdUnit, Medium).
  - **Mime associations:** `config_home/mimeapps.list` lines whose value list contains one of the app's desktop IDs (MimeAssoc, High). There is one finding for the file, with `mime_lines` set to those lines.
  - **Rules:** rule-resolved paths for the app are High, with kind derived from the parent directory.
- **Identifiers per app:**
  - **Strong:** `name`, desktop-ID stems, `rdns_id` and the last rdns segment when it is at least 4 characters. AppImage `name` is also strong.
  - **Weak:** `exec_names`, `wm_classes` and `display_name`.
  - Apply the global stoplist and minimum length.
- **Scoring:**
  - **High:** the candidate name equals a strong identifier exactly (for dot entries, the name without the leading `.`), or the candidate comes from a rule.
  - **Medium:**
    - `normalize_name(candidate) == normalize_name(id)` for any identifier, after also stripping a trailing version from the candidate with the regex-free rule "drop a final `-`/`_`/`.` segment that starts with a digit";
    - or an exact match on a weak identifier.
  - **Low:** `normalize_name(candidate)` contains `normalize_name(strong id)` and that ID is at least 4 characters.
- **Shared:** if a candidate is High or Medium for more than one app in the inventory, it is downgraded one level and `shared = true`. Low stays Low.
- **Preselection:** `preselected = confidence == High && !shared`.
- **Filtering:** candidates that are protected, or excluded by config, are dropped. Size is computed only for returned findings. `fingerprint` is taken at index time.

- [ ] **Step 1: Failing tests** (fixture home):
  - `exact_config_dir_is_high`: AppImage `GutCheck-0.19.0-x86_64.AppImage` with `~/.config/GutCheck` gives High and preselected.
  - `normalized_match_is_medium`: AppImage name `Grok_Bot` with `~/.config/Grok Bot` gives Medium, not preselected.
  - `dot_dir_exact`: `~/.hauler` for AppImage name `hauler` gives High. `~/.grokbot` for AppImage name `Grok_Bot` gives Medium, through a normalized match.
  - `rule_paths_high`: pacman `firefox` with `~/.mozilla/firefox` gives High via rule, and `~/.mozilla` itself is not a finding.
  - `flatpak_var_app_high`
  - `autostart_and_local_desktop_entry`
  - `icon_by_desktop_id`
  - `mimeapps_lines_collected`: two lines reference `gutcheck.desktop` and one does not, so `mime_lines.len() == 2`.
  - `systemd_user_unit_medium`
  - `shared_dir_downgraded` (Review Focus #2): pacman `chromium` and `chromium-widevine` both have exec name `chromium`. `~/.config/chromium` is High for `chromium`, but it is Medium for `chromium-widevine` through the weak exec name, so it is shared: Medium, `shared` set, not preselected.
  - `short_and_generic_names_ignored` (Review Focus #5): app name `go`, exec `code`. Neither `~/.config/go` nor `~/.cache/code-insiders` is a finding even with `show_low`.
  - `low_hidden_unless_show_low`: app `hauler` with `~/.cache/hauler-updater` gives Low, visible only with `show_low`.
  - `protected_never_found`: an app named `gnupg` does not get `~/.gnupg`.
  - `excluded_never_found`
  - `version_suffix_stripped`: `~/.config/Foo-2` for app `foo` gives Medium.
- [ ] **Step 2: Run** `cargo test -p crumbs-core --features test-support leftovers::` and expect FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** and expect PASS. Commit `feat(core): leftover matcher`.

---

### Task 8: Orphans

**Files:** Create `crumbs-core/src/leftovers/orphans.rs`.

**Interfaces:**
- `pub fn find_orphans(idx: &LeftoverIndex, show_low: bool) -> Vec<Finding>`: candidates of kinds Config, Cache, Data, State, DotDir, FlatpakData, SnapData, Autostart and DesktopEntry that match **no** installed app at any confidence and no matching rule.
- **Confidence:**
  - **High:** only when a builtin rule identifies the path's app and that app is not installed. `reason` = `belongs to <first package name in rule>, which is not installed`.
  - **Medium:** a FlatpakData directory whose ID is not installed, or an Autostart/DesktopEntry whose `exec_program` doesn't resolve (in trusted dirs, `~/.local/bin`, or as an absolute path).
  - **Low:** everything else: an unmatched dir in an XDG location or a dot entry. Low is hidden unless `show_low`.
- **Ignore list:** these names are never orphans, because they belong to the desktop or shell rather than an app:
  - **exact names:** `.bashrc .bash_profile .bash_logout .profile .zshrc .zshenv .zprofile .xinitrc .xprofile .Xresources .Xauthority .ICEauthority .gitconfig .dir_colors .face .face.icon .directory gtk-2.0 gtk-3.0 gtk-4.0 gtkrc gtkrc-2.0 dconf glib-2.0 fontconfig pulse pipewire kdedefaults user-dirs.dirs user-dirs.locale mimeapps.list environment.d systemd autostart menus xsettingsd`
  - **names that end in `rc`** directly under `config_home` (KDE settings files);
  - **names that start with `plasma`** directly under `config_home`.
- `preselected` is always false for orphans.

- [ ] **Step 1: Failing tests:**
  - `uninstalled_rule_app_is_high`: `~/.thunderbird` exists and thunderbird is not installed.
  - `installed_app_dir_not_orphan`
  - `unknown_dir_is_low_hidden_by_default`
  - `flatpak_var_app_without_install_is_medium`
  - `broken_autostart_is_medium`
  - `desktop_shell_files_ignored`: `.bashrc`, `kwinrc` and `gtk-3.0` are never listed.
  - `orphans_never_preselected`
- [ ] **Step 2: Run** and expect FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** and expect PASS. Commit `feat(core): orphan detection`.

---

### Task 9: Trash and history

**Files:** Create `crumbs-core/src/trash.rs` and `crumbs-core/src/history.rs`.

**Interfaces:**
- `pub struct Trashed { pub original: PathBuf /* in-system */, pub files_path: PathBuf /* in-system */, pub info_path: PathBuf /* in-system */ }` (Serialize and Deserialize).
- `pub fn trash_dir_for(env: &Env, in_system: &Path) -> Result<PathBuf, CoreError>`:
  - If the path is on the same device as `data_home`, use `{data_home}/Trash`.
  - Otherwise use `{topdir}/.Trash-{uid}`, where `topdir` is the highest ancestor still on the path's device. Create it with mode 0700.
  - If it cannot be created, or `topdir` is `/`, return an error.
- `pub fn move_to_trash(env: &Env, in_system: &Path) -> Result<Trashed, CoreError>`:
  1. Create `files/` and `info/` (0700) if missing.
  2. Pick a unique name: the basename, then `basename.2`, `.3`, and so on. A name is free only when neither `files/<name>` nor `info/<name>.trashinfo` exists.
  3. Write `info/<name>.trashinfo` first with `create_new`. Its content is `[Trash Info]\nPath=<percent-encoded in-system absolute path>\nDeletionDate=<local time %Y-%m-%dT%H:%M:%S>\n`.
  4. `rename` the item into `files/<name>`. If the rename fails, remove the `.trashinfo` and return the error.

  Percent-encoding keeps `A-Za-z0-9-._~/` and encodes everything else as `%XX`.
- `pub fn restore_from_trash(env: &Env, t: &Trashed) -> Result<(), CoreError>`:
  - It refuses with `CoreError::Io` and kind `AlreadyExists` if `original` exists, checked with `symlink_metadata`.
  - Otherwise it creates the missing parent dirs, renames back, then removes the `.trashinfo`.
- History:
  - `pub enum HistoryAction { Trash, Delete, EditFile, Restore }`
  - `pub struct HistoryEntry { pub id: String, pub batch: String, pub time: String /* RFC 3339 */, pub action: HistoryAction, pub path: PathBuf, pub trashed: Option<Trashed>, pub backup: Option<PathBuf>, pub size: u64, pub ok: bool, pub error: Option<String>, pub app_id: Option<String> }`
  - `pub fn new_batch_id() -> String` returns the current unix milliseconds as a decimal string. Entry IDs are `<batch>-<index>`.
  - `pub fn append(env: &Env, entries: &[HistoryEntry]) -> Result<(), CoreError>` creates `{state_home}/crumbs/` (0700) and opens with append+create. It writes one line per entry and then `sync_data`.
  - `pub fn read_history(env: &Env) -> Result<(Vec<HistoryEntry>, usize), CoreError>` returns entries in file order plus the number of unparseable lines it skipped. A missing file gives `Ok((vec![], 0))`.

- [ ] **Step 1: Failing tests:**
  - `trash_writes_info_and_moves`: the `.trashinfo` content matches, with `Path=` percent-encoding a space as `%20`.
  - `trash_name_collision` (Review Focus #3): two different `config` dirs give `config` and `config.2`.
  - `trash_failure_leaves_no_info`: trashing a non-existent path gives Err, and `info/` is empty.
  - `restore_round_trip`
  - `restore_refuses_occupied` (Review Focus #4, core half)
  - `trash_dir_other_device_selection`: test the topdir walk via `pub(crate) fn topdir_for(path, dev_of: impl Fn(&Path) -> Option<u64>) -> Option<PathBuf>` with a fake device function.
  - `history_append_and_read`
  - `history_skips_corrupt_line`
- [ ] **Step 2: Run** and expect FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** and expect PASS. Commit `feat(core): freedesktop trash and action history`.

---

### Task 10: Plan, executor and restore

**Files:** Create `crumbs-core/src/plan.rs`, `crumbs-core/src/executor.rs` and `crumbs-core/src/restore.rs`.

**Interfaces:**
- `pub enum Action { Trash { path: PathBuf }, Delete { path: PathBuf }, EditFile { path: PathBuf, remove_lines: Vec<String> } }` (Serialize, tag `"type"`, kebab-case).
- `pub struct PlannedAction { pub action: Action, pub size: u64, pub fingerprint: Fingerprint /* serde skip */, pub reason: String }`
- `pub struct Plan { pub app_id: Option<String>, pub actions: Vec<PlannedAction>, pub total_size: u64, pub notes: Vec<String> }`
- `pub struct PlanOptions { pub permanent: bool, pub keep_data: bool }`
- `pub fn build_plan(app: Option<&App>, findings: &[Finding], opts: &PlanOptions) -> Plan`:
  - MimeAssoc findings become `EditFile`.
  - Everything else becomes `Trash`, or `Delete` with `permanent`.
  - With `keep_data`, only Cache findings are kept.
  - For an AppImage app (when `app` is Some), the plan prepends a `Trash`/`Delete` of `app.location`, with its fingerprint taken now and reason `"AppImage file"`.
  - Notes: if `app.source` isn't AppImage, add `"the app itself stays installed (leftovers only)"`.
- `pub fn running_processes(env: &Env, app: &App) -> Vec<u32>` scans `env.sys("/proc")/<pid>`, ignoring unreadable entries. A pid matches when any of these hold:
  - its `exe` link target basename is in `exec_names`;
  - its `exe` equals the root-joined `location`;
  - its `environ` contains `APPIMAGE=<location>`.
- `pub struct ActionResult { pub action: Action, pub ok: bool, pub error: Option<String>, pub history_id: String }`
- `pub struct ExecReport { pub batch: String, pub results: Vec<ActionResult>, pub freed: u64 }`
- `pub fn execute(env: &Env, cfg: &Config, plan: &Plan) -> ExecReport`:
  - For each action in order, run `safety::verify` first, then act. Every attempt, success or failure, becomes a history entry, and they are appended in one `history::append` call at the end. If that append fails, every result gets the error text `"…; history not written"` appended.
  - **`EditFile`:**
    1. Copy the file to `<file>.crumbs-bak-<batch>`.
    2. Write the filtered content to a temp file in the same dir, then rename it over the original.
    3. Set the history entry's `backup` to the backup path.

    Removing a line means removing only lines exactly equal to one of `remove_lines`.
  - **`Delete`:** `remove_dir_all` or `remove_file`, on the root-joined path. It never follows symlinks; a symlink is removed as a link.
  - **`freed`:** the sum of sizes of the successful actions.
- `pub struct RestoreOutcome { pub entry_id: String, pub path: PathBuf, pub ok: bool, pub error: Option<String> }`
- `pub fn restore(env: &Env, selectors: &[String], dry_run: bool) -> Result<Vec<RestoreOutcome>, CoreError>`:
  - A selector is an entry ID, a batch ID (meaning all its entries), or an in-system path. A path matches the newest successful Trash entry with that original path.
  - Only successful `Trash` entries that have not already been restored can be restored. `EditFile` entries are restored by copying the backup back over the file.
  - Each attempt appends a `Restore` history entry unless `dry_run` is set.
  - An unknown selector gives an outcome with `ok: false` and error `no matching history entry`.

- [ ] **Step 1: Failing tests:**
  - `plan_trash_default_delete_permanent`
  - `plan_keep_data_only_cache`
  - `plan_mime_is_edit`
  - `plan_appimage_includes_file_first`
  - `execute_trashes_and_logs`
  - `execute_refuses_swapped_path`: Review Focus #1, end to end. The result is `ok == false`, `~/Documents/precious` still exists, and the history entry has `ok:false`.
  - `execute_partial_failure_continues`
  - `edit_file_backup_and_filter`
  - `running_process_detected_by_exe` and `running_process_detected_by_appimage_env`: fake `/proc/<pid>/exe` symlink and `environ` in the fixture.
  - `restore_by_batch`
  - `restore_by_path_newest`
  - `restore_partial_when_occupied` (Review Focus #4): two items, one original recreated, so one ok and one refused, and the refused item is still in the trash.
  - `restore_twice_is_noop_error`
  - `restore_edit_file_from_backup`
- [ ] **Step 2: Run** and expect FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** and expect PASS. Commit `feat(core): plan, executor and restore`.

---

### Task 11: CLI `apps show` leftovers and `orphans` (read-only)

**Files:** Create `crumbs-cli/src/cmd/orphans.rs`. Modify `crumbs-cli/src/cmd/apps.rs`, `crumbs-cli/src/main.rs`, `crumbs-cli/src/output.rs` and `crumbs-cli/tests/cli.rs`.

**Behaviour:**
- **Config and rules:** the CLI loads `Config` with `load_config` once per command. A parse error exits 1 with `error: config.toml: …`. Rules come from `load_rules`.
- **Rule errors:** user-rule errors become `warning: rules: <message>` on stderr. In JSON they become extra objects `{ "source": "rules", "message": … }` in the envelope's `errors` array, built in the CLI; `SourceKind` is not extended. They do not change the exit code.
- **`apps show <app> [--show-low]`:**
  - After the existing key/value lines, print a `Leftovers` table: `CONF  SIZE  KIND  PATH  NOTE`.
    - CONF is `high`, `medium` or `low`.
    - PATH is the display path (`~/…`).
    - NOTE is `shared` or empty.
  - The footer reads `N leftovers, <size> (M preselected)`.
  - JSON: `data.leftovers` is an array of `Finding`, so the fingerprint is not serialized.
- **`orphans [--show-low] [--json]`:** the same table without NOTE, but with a REASON column. With no `--clean`, it is read-only.
- Every path printed for humans goes through `display_path`.

- [ ] **Step 1: Failing tests** (CLI, fixture tree):
  - `apps_show_lists_leftovers`: a snapshot.
  - `apps_show_json_leftovers`
  - `orphans_lists_high_and_medium`
  - `orphans_show_low_includes_low`
  - `bad_user_rule_warns_but_succeeds`
- [ ] **Step 2: Run** and expect FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** and expect PASS. On the real system, run `cargo run -q -p crumbs-cli -- apps show GutCheck` and `cargo run -q -p crumbs-cli -- orphans`. These are read-only; paste the output in the report.
- [ ] **Step 5: Commit** `feat(cli): leftovers in apps show; orphans`.

---

### Task 12: CLI `uninstall`, `orphans --clean`, `history`, `restore`

**Files:** Create `crumbs-cli/src/prompt.rs`, `crumbs-cli/src/cmd/uninstall.rs`, `crumbs-cli/src/cmd/history.rs` and `crumbs-cli/src/cmd/restore.rs`. Modify `cmd/orphans.rs`, `main.rs` and `tests/cli.rs`.

**Behaviour:**
- **`uninstall <app>... [--leftovers-only] [--keep-data] [--show-low] [-i] [--dry-run] [--yes] [--permanent] [--force] [--json]`.** For each app:
  1. **Resolve** with `find_app`.
     - Exit 2 on not-found or ambiguous.
     - Exit 2 on a step-6-only match, printing `did you mean '<id>'?`. Add `pub fn find_app_exact` in core, which runs steps 1-5 only; the CLI tries it first and calls `find_app` only to build the suggestion.
  2. **Refuse risky states.**
     - Exit 2 for a non-AppImage app without `--leftovers-only`, using the message from Decision 2.
     - Exit 1 when the inventory has errors and `--force` is not given: `refusing: some sources failed (<list>); re-run with --force to continue`.
  3. **Collect findings** (`show_low` controls Low) and build the plan. Preselected findings plus the AppImage file are included. With `-i`, a `dialoguer::MultiSelect` over all findings opens, with defaults equal to `preselected`.
  4. **Preview.** Print the plan, `will move N items (<size>) to the trash` (or `permanently delete` with `--permanent`), and the notes. With `--dry-run`, stop here with exit 0.
  5. **Check for a running app.** If `running_processes` is non-empty, print `<app> is running (pid …)`.
     - Without `--force`, this needs a second confirmation when interactive.
     - When not interactive, it exits 1 unless `--force`.
  6. **Confirm.** Ask `Proceed? [y/N]` unless `--yes`, and follow the non-interactive rule in Global Constraints. Answering no exits 4.
  7. **Execute and print the report.** Each line is `✓ trashed ~/.config/GutCheck` or `✗ <path>: <error>`, followed by `freed <size> · undo with: crumbs restore <batch>`. Exit 3 if any action failed.
  - Several apps are planned and previewed together and confirmed once. JSON mode prints `{plans, report}` and requires `--yes` or `--dry-run` (exit 2 otherwise).
- **`orphans --clean`:** with `-i`, selection defaults to none; without `-i`, it requires `--select <path>...`. It never cleans every orphan implicitly. After that the flow is the same as uninstall from the preview step onward.
- **`history [--limit N] [--json]`:** newest first, grouped by batch. Each line is `<batch>  <time>  <app or "orphans">  N items  <size>  [restored k]`, and `--limit` counts batches (default 20).
- **`restore <selector>... [--dry-run] [--json]`:** prints per-item `✓`/`✗` and exits 3 on any failure.

- [ ] **Step 1: Failing tests** (CLI, fixture tree; `--yes` wherever it executes):
  - `uninstall_appimage_trashes_file_and_leftovers`: afterwards the AppImage and `~/.config/GutCheck` are in the fixture trash, and history has the batch.
  - `uninstall_dry_run_changes_nothing`
  - `uninstall_pacman_requires_leftovers_only_exit_2`
  - `uninstall_leftovers_only_pacman_keeps_package`
  - `uninstall_normalized_match_exit_2_with_suggestion`
  - `uninstall_refuses_on_source_errors_without_force`
  - `uninstall_non_interactive_without_yes_exit_2`
  - `uninstall_keep_data_only_cache`
  - `uninstall_json_requires_yes_or_dry_run`
  - `orphans_clean_select`
  - `history_lists_batch`
  - `restore_batch_round_trip`: uninstall, then restore, and the files are back.
  - `restore_occupied_exit_3` (Review Focus #4, CLI half)
- [ ] **Step 2: Run** and expect FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** `cargo test --workspace --all-features` and expect PASS.
- [ ] **Step 5: Real-system check (read-only only).** Run `cargo run -q -p crumbs-cli -- uninstall GutCheck --dry-run` and paste the output. **Do not run without `--dry-run` on the real system.**
- [ ] **Step 6: Commit** `feat(cli): uninstall, orphans --clean, history, restore`.

---

### Task 13: JSON schema doc and README

**Files:** Create `docs/json-schema.md`. Modify `README.md`.

- `docs/json-schema.md` documents the envelope, then each command's `data` shape:
  - `apps list`, `apps show` (including the not-found shape and `leftovers`), `doctor`, `orphans`, `uninstall` (`plans`, `report`), `history` and `restore`;
  - exit codes;
  - the ID formats from Decision 1;
  - the stability promise: fields may be added in schema v1, never removed or renamed.

  Every documented field must match the code. Generate examples by running the debug binary against a fixture (`CRUMBS_ROOT`) and pasting the real output.
- README: update the status to M2, add the new commands, and add a "Safety" paragraph covering trash-first, preview, `--dry-run` and `restore`.

- [ ] **Step 1: Write both docs** from real command output.
- [ ] **Step 2: Cross-check** each field named in `json-schema.md` against `grep -rn '"<field>"\|pub <field>'` in the crates; there must be no field the code lacks.
- [ ] **Step 3: Commit** `docs: json schema and README for M2`.
