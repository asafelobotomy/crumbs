# crumBS Milestone 4b (System Leftovers and Unmanaged Removal) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** crumbs can remove files that live outside home and belong to no package:
- `uninstall` offers an app's unowned `/etc/<name>` and `/opt/<name>` leftovers;
- `uninstall` of an `unmanaged:` app removes the app's own install directory when that directory is unambiguous;
- `orphans --system` lists unowned `/opt/<name>` directories that match no app.

All three go through the M4 pipeline: preview, confirm, one privileged batch, history.

**Architecture:**
- **Ownership:** a new `ownership` module keeps the package-owned files *and directories* from the pacman local database, and can be reloaded right before the batch runs.
- **Safety:** a new safety scope, `Scope::SystemTree`, covers direct children of `/etc` and `/opt`. It checks the root-owned parent chain, refuses symlinks, and refuses any target with a mount point at or below it (read from `/proc/self/mountinfo`).
- **Executor:** it turns a `SystemTree` `Delete` into the batch step `rm -rf --one-file-system -- <path>`. Right before the batch it re-checks the fingerprint, the mounts, package ownership and the pacman lock.
- **Findings and the CLI:**
  - Findings gain a `scope` field, so a system leftover is planned as a root delete.
  - The CLI renders these rows as permanent deletes that cannot be restored.

**Tech Stack:** The M4 stack. No new dependencies.

**Spec:** `docs/superpowers/specs/2026-11-08-crumbs-design.md`:
- §3: the Unmanaged row, "path-based removal of the install dir if it is unambiguous; otherwise leftovers only".
- §4: system search locations.
- §6: actions.
- §7: the last bullet, `rm -rf --one-file-system --` after the root-owned chain check.
- §8: safety.
- §13: milestone 4b.

**Scope and inherited follow-ups:** `docs/superpowers/notes/m4b-scope.md`.

## Decisions this plan makes (the spec leaves them open)

1. **What counts as unowned.**
   - A path is *owned* when the pacman local database lists it as a file, or as a directory (a `%FILES%` line ending in `/`).
   - A tree is *unowned* only when the root itself is not owned and no owned path lies below it. `/opt/foo` with any package file inside counts as owned.
   - Without a pacman database, every system leftover and system orphan is skipped, and the preview notes `system leftovers need the pacman database`. Other package managers come with M5.
2. **Where system leftovers come from.** Only direct children of in-system `/etc` and `/opt`. Never:
   - deeper paths;
   - symlinks;
   - anything named in `ETC_NEVER`, a built-in list of names that are never offered even when unowned: `passwd shadow group gshadow sudoers sudoers.d ssh ssl ca-certificates pam.d security systemd fstab crypttab hosts hostname machine-id locale.conf localtime vconsole.conf adjtime ld.so.cache ld.so.conf ld.so.conf.d os-release pacman.conf pacman.d mkinitcpio.conf mkinitcpio.d modprobe.d modules-load.d sysctl.d udev X11 NetworkManager polkit-1 dbus-1 default skel profile profile.d environment subuid subgid kernel cmdline.d secureboot crypttab.initramfs machine-info resolv.conf`;
   - names ending in `-` (`passwd-`, `shadow-` and the like);
   - names ending in `.bak`, `.old`, `.pacnew` or `.pacsave`. These files are the admin's, so crumbs never offers them, as a leftover or as an orphan.

   On the dev machine, `/etc` holds 23 unowned entries (`machine-id`, `locale.conf`, `openvpn`, `brave`, `Proton`, …). Most of them are system state, which is why `/etc` never produces orphans (Decision 4).
3. **Matching and confidence for system leftovers.** They are never High and never pre-selected: `--yes` alone never deletes one. The user picks one with `-i`.
   - **Medium:** an exact, case-sensitive name match of the entry against:
     - the app's `name`;
     - a desktop-id stem;
     - the reverse-DNS id;
     - an `Exec` basename;
     - a curated-rule path.
     Rules may list `/etc/<name>` and `/opt/<name>`; any other absolute path in a rule stays invalid, as today.
   - **Low:** a match on the normalised `NameKey`. Low is shown only with `--show-low`. Token and substring matches are never used outside home.
   - **Shared:** a name that matches more than one installed app is downgraded and marked `shared`, as in home.
4. **System orphans.**
   - `orphans --system` lists unowned `/opt/<name>` directories that:
     - match no detected app (the same matching as Decision 3);
     - are not the `location` of any detected app.
   - They are Low, never pre-selected, and acted on only through `--select <path>` or `-i`.
   - `/etc` is never searched for orphans.
   - Plain `orphans` is unchanged (home only), and `--user --system` is exit 2, as for `clean`.
5. **When an unmanaged app's own files can be removed.** Take the app's `Exec` program, canonicalised (symlinks followed).
   - **Under `/opt/<dir>/`:** the target is `/opt/<dir>`, scope `SystemTree`, a root delete. It is offered only when all of these hold:
     - (a) the tree is unowned (Decision 1);
     - (b) no other detected app has its program, or its `location`, under `/opt/<dir>`;
     - (c) `<dir>` matches the app by exact name or `NameKey` (Decision 3's sets, case-insensitive). `/opt/google` for Chrome fails this check.
   - **Under `~/opt/<dir>/` or `~/Applications/<dir>/`:** the target is that directory, scope `Home`, trashed, with the same checks (a)–(c).
   - **A single regular file directly in `~/.local/bin` or `~/bin`:** the target is that file, scope `Home`, trashed, and its basename must match the app.
   - **Anything else** is refused:
     - a program in protected `/usr`;
     - `/usr/local`;
     - a vendor or shared directory;
     - a name mismatch.

     The refusal reads `removing this unmanaged app's files isn't possible: <reason>; use --leftovers-only` (exit 2). The `<reason>` strings are pinned in Task 5.
   - When the program was reached through a symlink in a protected path (`/usr/bin/foo` → `/opt/foo/foo`), the preview notes `<link> is left in place (protected path)`.
6. **Root tree deletion.**
   - **Batch step:** `rm -rf --one-file-system -- <in-system path>`, with `rm` from `resolve_trusted`. A non-directory target uses the same step.
   - **Re-checks:** right before the batch runs, as the user, every `SystemTree` target is checked again. A failure refuses that target (✗, no step):
     - `safety::verify` (location, chain, not a symlink, fingerprint);
     - `mounts_at_or_below` must return no mount;
     - the reloaded `OwnedIndex` must find nothing owned at or below the target;
     - no `/var/lib/pacman/db.lck` may exist.
   - **No ctime for directories:** directory fingerprints stay `(dev, ino, is_symlink)`. The chain from `/` down to the parent is root-only, so only root can swap the entry, and the mount and ownership re-checks cover the changes that matter between scan and run.
7. **The user is told it is permanent.**
   - Human rows read `delete (as root)`, followed by the `as root:` command line.
   - A plan with any `SystemTree` delete carries the note `system files are deleted permanently with administrator rights and cannot be restored`.
   - History records them as `delete` with `scope: "system-tree"`. `restore <id>` answers `system files deleted as root cannot be restored`, and batch restores skip them.
   - `--permanent` makes no difference for these rows, which are always deleted, never trashed.
8. **Carried follow-ups.**
   - Task 7 takes the trash-root and process-matching items the scope note asks for before system roots ship.
   - Task 8 takes three user-visible M3 leftovers and splits `act.rs`.
   - Everything else in `m4b-scope.md` stays deferred and is listed in Task 9's follow-ups note.

## Global Constraints

- **Earlier constraints:** all M1–M4 global constraints still apply. See the M4 plan, `docs/superpowers/plans/2026-11-08-crumbs-m4-privileged.md`, "Global Constraints".
- **No new dependencies.**
- **Never trigger a real privilege escalation in tests or verification:**
  - Tests use `FakePrivilegedRunner` or the debug `CRUMBS_TEST_PRIV_RUNNER`, under fake roots only.
  - **Never** run `cargo test --release`.
  - Real-system checks are limited to:
    - `apps list`;
    - `uninstall <app> --dry-run`;
    - `orphans --system` without `--select`;
    - `pacman -Qo`.
  - **Never** run `uninstall`, `orphans --select` or `orphans -i` without `--dry-run` on the real system.
- **Never touch** the real `~/.local/state/crumbs`, `~/.local/share/crumbs`, `/etc` or `/opt`, other than by reading them.
- **Test-only hooks:** "root-owned" in fixtures means `Env::system_owner_uid`, through `set_system_owner_uid`. Mount data in fixtures comes from `<root>/proc/self/mountinfo`.
- **Paths:**
  - Every new path is in-system, canonical, and checked by `safety::verify` immediately before its action.
  - Display strings go through `display_path`, and in the CLI through `render::sanitise`.

## Review Focus

1. **A live app's unowned config directory.** `/etc/brave` exists while Brave is installed. It must never be pre-selected, and must never appear in `orphans --system`. Pinned by `system_leftover_of_installed_app_is_shared_and_not_preselected` (Task 4) and `orphans_system_never_lists_etc` (Task 6).
2. **A bind mount inside `/opt/<name>`.** It must be refused before the batch runs, and no `rm` step may run for it. Pinned by `system_tree_delete_refused_when_mount_below` (Task 3).
3. **A package that starts owning a path between scan and run.** A package installed between the preview and the run puts a file under the target. The target must be refused. Pinned by `system_tree_delete_refused_when_owned_since_scan` (Task 3).
4. **A shared vendor directory.** `/opt/google` with the app Google Chrome, or two apps under one `/opt/<dir>`. The full uninstall must be refused with a reason, and `--leftovers-only` must still work. Pinned by `unmanaged_vendor_dir_refused` and `unmanaged_dir_shared_by_two_apps_refused` (Task 5).
5. **An unreadable system directory.** `/etc/Proton` has mode 0700 and is root-owned, so the user cannot read it. It must still be listed, with size unknown, and be deletable. The mount check must not need to read inside it. Pinned by `unreadable_system_leftover_listed_with_unknown_size` (Task 4) and `mount_check_does_not_read_the_tree` (Task 2).

---

### Task 1: Owned files and directories

**Files:**
- Create: `crates/crumbs-core/src/ownership.rs`.
- Modify:
  - `sources/pacman.rs`: `owned_files` keeps directories in a second set.
  - `sources/mod.rs`: `SourceListing` gains `owned_dirs`.
  - `inventory.rs`: `Inventory.owned: OwnedIndex`.
  - `lib.rs`.

**Interfaces:**
- Produces:
  - `pub struct OwnedIndex`, with:
    - `files: HashSet<PathBuf>`;
    - `dirs: BTreeSet<PathBuf>`;
    - an extra `BTreeSet` of all owned paths for range queries.
  - `OwnedIndex::owns(&self, p: &Path) -> bool`.
  - `OwnedIndex::owns_at_or_below(&self, p: &Path) -> bool`.
  - `OwnedIndex::is_empty(&self) -> bool`.
  - `ownership::load_pacman(env: &Env) -> Result<Option<OwnedIndex>, CoreError>`. It returns `None` when `/var/lib/pacman/local` is missing.
  - `Inventory.owned: OwnedIndex`. The existing unmanaged detection keeps using the files only.
- Range query: `owns_at_or_below` uses `range(p..)` and stops at the first entry that does not `starts_with(p)`, comparing whole components. `/opt/foo` must not match `/opt/foobar`.

- [ ] **Step 1: Write the failing tests** in `ownership.rs`:
  - `dirs_from_files_section_are_owned`: a fixture package whose `%FILES%` lists `opt/foo/` and `opt/foo/bin/foo`. Assert `owns("/opt/foo")` and `owns_at_or_below("/opt/foo")`.
  - `prefix_is_component_wise`: an owned `/opt/foobar/x`. Assert `!owns_at_or_below("/opt/foo")`.
  - `unowned_tree_is_unowned`: `/opt/inky` with no package entry gives false for both queries.
  - `no_pacman_db_is_none`: a fixture without `var/lib/pacman/local` makes `load_pacman` return `Ok(None)`.
- [ ] **Step 2: Run** `cargo test -p crumbs-core ownership`. Expected: FAIL (no module).
- [ ] **Step 3: Implement.** The pacman listing fills both sets. Directory lines lose their trailing `/`, and get a leading `/`.
- [ ] **Step 4: Run** `cargo test --workspace`. Expected: PASS. The existing `owned_paths_cover_all_packages` still passes.
- [ ] **Step 5: Commit** `feat(core): owned-directory index from the pacman database`.

### Task 2: `Scope::SystemTree` and the mount check

**Files:**
- Modify: `crates/crumbs-core/src/safety.rs`.
- Create: `crates/crumbs-core/src/mounts.rs`.
- Tests in each file.

**Interfaces:**
- Consumes: `check_system_chain`, `check_real_dir`, `is_protected_by` and `is_excluded`, all in `safety.rs`.
- Produces:
  - `Scope::SystemTree`, serde `system-tree`.
  - `pub const ETC_NEVER: &[&str]`, holding Decision 2's list.
  - `pub fn is_system_leftover_name(name: &OsStr) -> bool`. It is false for `ETC_NEVER` names, for names ending in `-`, `.bak`, `.old`, `.pacnew` or `.pacsave`, and for names starting with `.`.
  - `mounts::mounts_at_or_below(env: &Env, in_system: &Path) -> Result<Vec<PathBuf>, CoreError>`. It reads `env.sys("/proc/self/mountinfo")`, takes field 5 (the mount point) of each line, decodes the octal escapes `\040`, `\011`, `\012` and `\134`, maps the mount points to in-system paths with `env.unsys`, and returns those equal to or below `in_system`, comparing whole components.
  - An unreadable or unparsable mountinfo is an `Err`, and callers fail closed.
- **Rules for `SystemTree`:**
  - `is_allowed_target`: the parent is exactly `/etc` or `/opt`, the name passes `is_system_leftover_name`, and the path is neither protected nor excluded.
  - `check_location`: `check_system_chain(parent)`.
  - `verify_now`: the target is a real directory or a regular file, not a symlink. Nothing else.

- [ ] **Step 1: Write the failing tests:**
  - `system_tree_allows_direct_children_only`:
    - `/opt/inky` and `/etc/inky` are allowed;
    - `/opt/inky/sub`, `/opt` and `/var/foo` are not;
    - `/etc/machine-id`, `/etc/passwd-` and `/etc/foo.pacsave` are not.
  - `system_tree_refuses_symlink_and_writable_parent`:
    - a symlink `/opt/x` gives `Ineligible("is a symlink")`;
    - `/opt` with mode 0o777 gives `"is writable by group or others"`.
  - `system_tree_verify_accepts_root_owned_chain`: in a fixture with `set_system_owner_uid(uid)`, `verify` passes for a directory, then gives `Changed` after `/opt/inky` is replaced by a new directory with a different inode.
  - `mountinfo_parsing_and_escapes`: a fixture mountinfo with `/opt/inky/data`, `/opt/inky\040x` and `/opt/inkyz`. For `/opt/inky`, the result is `[/opt/inky/data]`.
  - `mount_check_does_not_read_the_tree`: the target directory has mode 0o000, and its mountinfo shows a mount below it. The function returns that mount and never opens the directory. Restore the mode in a drop guard.
  - `missing_mountinfo_fails_closed`: the function returns `Err`.
- [ ] **Step 2: Run** `cargo test -p crumbs-core safety mounts`. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** `cargo test --workspace`. Expected: PASS.
- [ ] **Step 5: Commit** `feat(core): system-tree safety scope and mount check`.

### Task 3: Executor — root tree delete with re-checks

**Files:**
- Modify:
  - `crates/crumbs-core/src/executor/mod.rs`: `is_privileged`, the step builder at the System `Delete` arm, and `recheck_scope`.
  - `history.rs`: scope recorded on `delete`.
  - `restore.rs`.
- Tests: `executor/tests.rs`, `restore.rs` tests.

**Interfaces:**
- Consumes:
  - `ownership::load_pacman` (Task 1);
  - `mounts::mounts_at_or_below` and `Scope::SystemTree` (Task 2);
  - `removal::lock_path(env)`, which is `pub(crate)` in `removal/mod.rs`.
- Produces:
  - A `SystemTree` `Delete` becomes the step `<rm> -rf --one-file-system -- <path>`.
  - `recheck_scope` for `SystemTree` returns `Err` with one of these exact messages:
    - `"a mount point is below it"`;
    - `"a package owns files in it now"`;
    - `"pacman is busy (database lock present)"`;
    - `"the package database could not be read"`.
  - `HistoryEntry` gets `scope: Option<Scope>` on `delete`, skipped when `Home`.
  - `restore::MSG_SYSTEM_TREE = "system files deleted as root cannot be restored"`.

- [ ] **Step 1: Write the failing tests:**
  - `system_tree_delete_runs_one_rm_rf_step`:
    - The `FakePrivilegedRunner` records a script containing `'-rf' '--one-file-system' '--' '/opt/inky'`.
    - The report is ok.
    - The fixture directory is gone.
  - `system_tree_delete_refused_when_mount_below`: a mountinfo line for `/opt/inky/data` gives ✗ `a mount point is below it`. The script holds no step for it, and the directory still exists.
  - `system_tree_delete_refused_when_owned_since_scan`: the plan is built, then a package `files` entry for `opt/inky/new` is added to the fixture database. The target is refused with `a package owns files in it now`.
  - `system_tree_delete_refused_while_pacman_locked`: with `db.lck` present, the target is refused.
  - `system_tree_history_and_restore`: the history line has `"scope":"system-tree"`. `restore <id>` gives `MSG_SYSTEM_TREE`. A batch restore skips the entry.
- [ ] **Step 2: Run** `cargo test -p crumbs-core executor restore`. Expected: FAIL.
- [ ] **Step 3: Implement.** Load the owned index once per batch, not once per target.
- [ ] **Step 4: Run** `cargo test --workspace`. Expected: PASS.
- [ ] **Step 5: Commit** `feat(core): root tree deletes with mount, ownership and lock re-checks`.

### Task 4: System leftovers of an app

**Files:**
- Create: `crates/crumbs-core/src/leftovers/system.rs`.
- Modify:
  - `leftovers/mod.rs`: `Finding.scope: Scope` (default `Home`, serialised `scope`) and new `FindingKind`s.
  - `rules.rs`: allow `/etc/<name>` and `/opt/<name>` paths.
  - `plan.rs::build_plan`: a `SystemTree` finding becomes `Delete` with `scope: SystemTree`, ignoring `permanent`, and adds the Decision 7 note.
  - `leftovers/matcher.rs`.
- Tests: `leftovers/tests.rs`, `plan.rs` tests.

**Interfaces:**
- Consumes: `OwnedIndex` (Task 1); `Scope::SystemTree` and `is_system_leftover_name` (Task 2); `NameKey` and the shared-downgrade logic in `leftovers/score.rs`.
- Produces:
  - `FindingKind::SystemConfig` for `/etc`, serde `system-config`.
  - `FindingKind::SystemData` for `/opt`, serde `system-data`.
  - `leftovers::find_system_leftovers(env: &Env, cfg: &Config, idx: &LeftoverIndex, app: &App, show_low: bool) -> Vec<Finding>`. Every finding it returns has `preselected: false`, `scope: SystemTree` and a confidence of at most Medium.
  - `LeftoverIndex::build` takes the index from `inv.owned`.
  - `plan::NOTE_SYSTEM_DELETE = "system files are deleted permanently with administrator rights and cannot be restored"`.
- **Size:** `disk_usage`. An unreadable tree gives `size.errors > 0`. Task 6's renderer shows a finding with `errors > 0` and `bytes == 0` as `-`.

- [ ] **Step 1: Write the failing tests:**
  - `system_leftover_exact_name_is_medium_not_preselected`: an app named `inky` with unowned `/opt/inky` and `/etc/inky`. Both are found as Medium with `preselected: false` and `scope: SystemTree`.
  - `owned_system_dir_not_offered`: when `/opt/foo` holds a package file, nothing is found.
  - `normalised_match_is_low_and_hidden`: `/opt/Inky-App` for the app `inky` is returned only with `show_low`.
  - `etc_never_names_not_offered`: an app named `ssh` with unowned `/etc/ssh` gives nothing.
  - `system_leftover_of_installed_app_is_shared_and_not_preselected`: two detected apps, `brave` and `brave-beta`, both matching `/etc/brave`. The finding is `shared` and Low.
  - `unreadable_system_leftover_listed_with_unknown_size`: `/etc/proton` with mode 0o000 is listed with `size.errors > 0`. Restore the mode in a drop guard.
  - `rule_may_name_etc_and_opt_children`: a rule path `/etc/foo` gives a Medium finding. A rule path `/etc/foo/bar` is rejected as invalid.
  - `build_plan_system_finding_is_root_delete`: with `permanent: false`, the plan holds `Delete` with `scope: SystemTree`, and the note is present.
- [ ] **Step 2: Run** `cargo test -p crumbs-core leftovers plan`. Expected: FAIL.
- [ ] **Step 3: Implement.** When `Inventory.owned` is empty because there is no pacman database, return nothing.
- [ ] **Step 4: Run** `cargo test --workspace`. Expected: PASS.
- [ ] **Step 5: Commit** `feat(core): system leftovers in /etc and /opt`.

### Task 5: Removal planning for unmanaged apps

**Files:**
- Modify:
  - `crates/crumbs-core/src/removal/mod.rs`: `SourceKind::Unmanaged` no longer returns `MSG_UNMANAGED` outright.
  - `sources/unmanaged.rs`: keep the canonical program on the `App` as `program: Option<PathBuf>`, serde-skipped, and the link path when it differs.
- Create: `removal/unmanaged.rs`.
- Test: `removal/tests.rs`.

**Interfaces:**
- Consumes: `Inventory` (all apps, and `owned`); `Scope::SystemTree` and `safety::fingerprint`; the name sets from Task 4.
- Produces:
  - `removal::plan_unmanaged(env: &Env, inv: &Inventory, app: &App) -> Result<UnmanagedTarget, RemovalError>`.
  - `pub struct UnmanagedTarget`, with these fields:
    - `path: PathBuf`;
    - `scope: Scope` (`SystemTree` or `Home`);
    - `fingerprint: Fingerprint`;
    - `size: DiskUsage`;
    - `left_link: Option<PathBuf>`.
  - `RemovalError::Unsupported(String)` with the text `removing this unmanaged app's files isn't possible: <reason>; use --leftovers-only`. The exact reasons:
    - `its program is in a protected or shared location`;
    - `<dir> belongs to a package`;
    - `<dir> is shared with <other app display name>`;
    - `<dir> does not match the app's name`.
- **Planning:** `plan.rs` gets `build_plan_unmanaged`, or `build_plan` gains an `Option<&UnmanagedTarget>`; pick one and keep the AppImage path unchanged.
  - The target action comes first: `Delete` for `SystemTree`, `Trash` (or `Delete` with `--permanent`) for `Home`.
  - Every leftover depends on it, as for AppImage.
  - When `left_link` is set, add the note `<link> is left in place (protected path)`.

- [ ] **Step 1: Write the failing tests:**
  - `unmanaged_opt_dir_planned_as_root_delete`: the program is `/opt/inky/Inky`, `/opt/inky` is unowned and no other app uses it. The target is `/opt/inky` with scope `SystemTree`.
  - `unmanaged_vendor_dir_refused`: for the app `google-chrome-dev`, `/opt/google/chrome/chrome` gives `does not match the app's name`.
  - `unmanaged_dir_shared_by_two_apps_refused`: two apps both run programs under `/opt/suite/`. The result is `is shared with`.
  - `unmanaged_owned_dir_refused`: a package owns a file under `/opt/inky`. The result is `belongs to a package`.
  - `unmanaged_home_dir_trashed`: the program is `~/opt/foo/foo`. The target is `~/opt/foo` with scope `Home`.
  - `unmanaged_local_bin_file_trashed`: the program is `~/.local/bin/foo`, and the target is that file.
  - `unmanaged_usr_local_refused`: the program is `/usr/local/bin/foo`. The result is `its program is in a protected or shared location`.
  - `unmanaged_symlinked_program_notes_link`: `/usr/bin/inky` links to `/opt/inky/Inky`. The target is `/opt/inky`, with `left_link = /usr/bin/inky`.
- [ ] **Step 2: Run** `cargo test -p crumbs-core removal`. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** `cargo test --workspace`. Expected: PASS.
- [ ] **Step 5: Commit** `feat(core): path-based removal planning for unmanaged apps`.

### Task 6: CLI — uninstall and `orphans --system`

**Files:**
- Modify:
  - `crates/crumbs-cli/src/cmd/uninstall.rs`;
  - `cmd/orphans.rs`;
  - `cmd/render.rs`;
  - `crates/crumbs-core/src/leftovers/orphans.rs`: add `find_system_orphans(env, cfg, idx) -> Vec<Finding>`.
- Test: `tests/cli.rs`.

**Interfaces:**
- Consumes: Tasks 1–5. The CLI's `act::apply` through `execute_many`, unchanged.
- Produces:
  - **`uninstall`:**
    - It adds `find_system_leftovers` to the choices.
    - For an `unmanaged:` app it calls `plan_unmanaged`. On `Unsupported` it exits 2 with the message.
    - `--leftovers-only` still works for every unmanaged app.
  - **`orphans`** gains `--system` and `--user`. Rules:
    - `--user --system` is exit 2 `use either --user or --system`;
    - `--system` lists only system orphans;
    - `--select <path>` selects only orphans of the chosen kind.
  - **`render`:**
    - A `SystemTree` `Delete` row reads `delete (as root)`, followed by the existing `as root:` line.
    - A finding with `size.errors > 0` and `bytes == 0` shows its size as `-`.
  - **JSON:** findings carry `scope`. The JSON shapes are documented in Task 9.

- [ ] **Step 1: Write the failing tests:**
  - `uninstall_lists_system_leftovers_not_preselected`: `uninstall inky --yes` with the `sh` runner deletes nothing in `/opt` or `/etc`. The preview lists both, unselected.
  - `uninstall_unmanaged_removes_opt_dir_as_root`:
    - `uninstall unmanaged:inky.desktop --yes` with the `sh` runner removes `/opt/inky`, in one batch.
    - The history scope is `system-tree`.
    - The human output shows `delete (as root)` and the Decision 7 note.
  - `uninstall_unmanaged_vendor_dir_exit_2_and_leftovers_only_works`.
  - `orphans_system_lists_unowned_opt_dirs`: unowned `/opt/old-app` that matches no app is listed with `--system`, and is absent without it.
  - `orphans_system_never_lists_etc`: an unowned `/etc/old-app` is never listed.
  - `orphans_system_select_deletes_as_root`: `orphans --system --select /opt/old-app --yes` with the `sh` runner removes it.
  - `orphans_user_and_system_conflict_exit_2`.
  - `system_delete_denied_exit_5_lists_rm_rf`: with the `deny` runner, `manual_commands` holds the `rm -rf --one-file-system -- /opt/inky` line.
- [ ] **Step 2: Run** `cargo test -p crumbs-cli`. Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** `cargo test --workspace`. Expected: PASS.
- [ ] **Step 5: Run the real-system checks.** These are read-only; paste their output into the report:
  - `cargo run -q -p crumbs-cli -- apps list --source unmanaged`;
  - `cargo run -q -p crumbs-cli -- orphans --system`. Expect `/opt/inky` on the dev machine.
  - `cargo run -q -p crumbs-cli -- uninstall <the listed unmanaged id> --dry-run`.

  **Never** run any of them without `--dry-run` or with `--select`.
- [ ] **Step 6: Commit** `feat(cli): system leftovers, unmanaged uninstall and orphans --system`.

### Task 7: Trash roots and process matching (before system roots ship)

**Files:**
- Modify: `crates/crumbs-core/src/trash.rs`, `restore.rs` (`valid_trash`), `safety.rs` (`HomeTrash`) and `executor/mod.rs` (`running_processes`).
- Tests: in those files.

**Interfaces:**
- **`HomeTrash`:** the `Trash` directory must also lie under the in-system home directory. A `data_home` outside home gives `OutsideAllowedRoots`.
- **Trash topdir:** `valid_trash` accepts a topdir trash root only when all of these hold:
  - `<topdir>/.Trash-<uid>` is a real directory, owned by the user, with mode 0o700;
  - its `files` and `info` are real directories owned by the user;
  - or the freedesktop shared `<topdir>/.Trash/<uid>` form applies, with `.Trash` sticky and not a symlink.
- **Process matching:** an `Exec` program whose basename is an Electron-style launcher script (`claude` in the M3 report) is matched through `/proc/<pid>/cmdline`, whose argv0 or argv1 is the app's canonical program path. Cover the case with a test before changing the code. The commit message says whether aa5a04b already covered it.

- [ ] **Step 1: Write the failing tests:**
  - `home_trash_outside_home_refused`;
  - `topdir_trash_needs_private_owner_dir`, with modes 0o755, wrong owner, symlink and the valid case;
  - `shared_dot_trash_needs_sticky`;
  - `running_detects_script_launched_electron_app`.

  If the last one passes before any change, keep it as a regression test and say so in the report.
- [ ] **Step 2: Run** them. Expected: FAIL, except possibly the last one.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** `cargo test --workspace`. Expected: PASS.
- [ ] **Step 5: Commit** `fix(core): stricter trash roots and script-launched app detection`.

### Task 8: User-visible M3 leftovers and the `act.rs` split

**Files:**
- Modify:
  - `crates/crumbs-core/src/restore.rs` (`select`);
  - `plan.rs` or `cmd/uninstall.rs` (`--keep-data` mime);
  - `config.rs` (`glob_match`);
  - `crates/crumbs-cli/src/cmd/act.rs`, split into `act/mod.rs`, `act/privilege.rs`, `act/signals.rs` and `act/test_runner.rs`. Move code only, with no behaviour change.

**Interfaces:**
- **Restore:** `restore <batch>` on a delete-only batch prints `nothing in this batch can be restored` and exits 0, instead of `no matching history entry` with exit 2.
- **`--keep-data`:** when the app's launcher is removed, its `mimeapps.list` lines go too: the `MimeAssoc` finding is kept.
- **`glob_match`:**
  - `**` is matched in linear time, with no exponential backtracking: a test of 30 `**` segments against a 200-character path finishes in under 50 ms.
  - `.cache/**` matches `.cache` itself.

- [ ] **Step 1: Write the failing tests:**
  - `restore_delete_only_batch_says_nothing_restorable`;
  - `keep_data_still_removes_mime_lines_of_removed_launcher`;
  - `glob_double_star_is_linear`;
  - `glob_dir_double_star_matches_dir`.
- [ ] **Step 2: Run.** Expected: FAIL.
- [ ] **Step 3: Implement** the three fixes. Then do the `act.rs` split as a separate commit, with all tests passing before and after.
- [ ] **Step 4: Run** `cargo test --workspace`, then `cargo clippy --workspace --all-targets --all-features -- -D warnings`. Expected: PASS.
- [ ] **Step 5: Commit** two commits: `fix: restore, keep-data and glob follow-ups`, then `refactor(cli): split act.rs`.

### Task 9: Docs

**Files:**
- Modify:
  - `docs/json-schema.md`: the `system-tree` scope, `system-config` and `system-data` kinds, `orphans --system`, the unmanaged refusal texts, and `restore` of system deletes.
  - `README.md`: system leftovers, unmanaged uninstall and their limits, and the Safety paragraph covering the root-owned chain, the mount check, the ownership re-check and `--one-file-system`.
  - Spec §13: 4b marked done.
- Create: `docs/superpowers/notes/2026-11-08-m4b-review-followups.md`, which lists every `m4b-scope.md` item M4b did not take.

- [ ] **Step 1: Write the docs.** Generate the examples with the debug binary against a fixture fake root and `CRUMBS_TEST_PRIV_RUNNER=sh`.
- [ ] **Step 2: Cross-check them** against the serde structs and the CLI strings.
- [ ] **Step 3: Commit** `docs: system leftovers and unmanaged removal`.
