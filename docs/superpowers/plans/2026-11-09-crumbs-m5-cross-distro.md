# crumBS Milestone 5 (Cross-distro) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** crumbs works on Debian/Ubuntu (dpkg/apt), Fedora (rpm/dnf) and Snap the way it already works on Arch. Concretely:
- it lists those apps and their owned files;
- it uninstalls them through one escalation, with a simulated, exact preview;
- it cleans their package caches and orphaned packages;
- a podman distro matrix proves all of this inside real containers.

**Architecture:**
- **Package database:** one `PackageDb` dispatch in `ownership` builds the owned-files-and-directories index from whichever database is present: pacman, dpkg or rpm. M4b's system leftovers and unmanaged checks then work on every distro.
- **Sources:** new `sources::{dpkg, rpm, snap}` sit beside pacman.
- **Removal:** new `removal::{apt, dnf, snap}` modules follow the pacman pattern. They simulate as the user, run the exact simulated list as root, and post-check the result.
- **System categories:** `pkg-cache` and `orphan-pkgs` gain apt and dnf variants.
- **Running as root:** an already-root run executes the privileged batch directly. The container end-to-end tests need this, and so do users who run crumbs as root.

**Tech Stack:** the M4c stack. No new dependencies.

**Spec:**
- `docs/superpowers/specs/2026-11-08-crumbs-design.md`: §3 (source table), §5 (system scope table), §6 (simulation), §11 (testing), §13 (milestone 5).
- Inherited follow-ups: `docs/superpowers/notes/*-review-followups.md`.

## Decisions this plan makes (the spec leaves them open)

1. **Which sources are on.** A source is available when its database or tool exists:
   - dpkg: `/var/lib/dpkg/status`;
   - rpm: `rpm` in the trusted dirs;
   - Snap: `snap` in the trusted dirs.

   Several can be on at once, for example Ubuntu with Snap. The package database used for ownership is the first one present in the order pacman, dpkg, rpm.
2. **dpkg listing.** Parse `/var/lib/dpkg/status` directly, with no command. Use these fields:
   - **Package:** the name.
   - **Status:** the package must be `install ok installed`.
   - **Version.**
   - **Architecture.**
   - **Installed-Size:** in KiB, so the install size is ×1024.

   Owned paths come from `/var/lib/dpkg/info/<pkg>.list` and `<pkg>:<arch>.list`. Every listed path counts as owned; dpkg lists directories as plain paths. Read the files as bytes, as in M4b Task 1. An unreadable `.list` makes the index incomplete.
3. **rpm listing.** Run the trusted `rpm` twice:
   - `rpm -qa --qf '%{NAME}\t%{EPOCHNUM}:%{VERSION}-%{RELEASE}.%{ARCH}\t%{SIZE}\n'` lists the packages.
   - `rpm -qa --qf '[%{FILENAMES}\n]'` gives every owned path in one call.

   The rpm database is binary and is never parsed directly. A failed query is a source error; the index is then incomplete.
4. **apt removal.**
   - **Simulate** as the user: `apt-get -s purge <pkg>`. Parse every `Purg <name> [<ver>]` and `Remv <name> [<ver>]` line.
   - **Refuse** with `Refused` when:
     - the simulation shows an `Inst` or `Conf` of something not being removed;
     - it prints `E:`. The refusal text is apt's `E:` lines.
   - **Execute** `apt-get purge -y --no-install-recommends <exact simulated names>` as root.
   - **Busy:** apt's lock is an fcntl lock, not a file, so the lock-file check does not apply. apt's own "Could not get lock" output becomes the error `apt is busy (another package manager is running)`.
   - **Post-check** by re-reading `/var/lib/dpkg/status`. A package still `install ok installed` is still installed. A package left in `deinstall ok config-files` state also counts as not removed, because purge should leave none.
5. **dnf removal.**
   - **Simulate** as the user: `dnf remove --assumeno <pkg>`. The exit status is non-zero by design. Parse the transaction table's removal section.
     - Fedora ≥ 41 uses dnf5. dnf4 lists packages under `Removing:` and `Removing unused dependencies:`; dnf5 under `Removing packages:` and `Removing unused dependencies:`.
     - Detect the dialect from the table headers.
     - Anything unparsable is `Refused("could not read dnf's plan")`.
   - **Execute** `dnf remove -y --setopt=clean_requirements_on_remove=False <exact simulated names>`, so dnf can never remove more than the preview showed.
   - **Post-check** with `rpm -q <names>`.
   - zypper and plain `rpm -e` are out of scope (follow-up).
6. **Snap.**
   - **Listing:** `snap list --unicode=never`, columns `Name Version Rev Tracking Publisher Notes`. Rows whose Notes contain `base`, `core`, `snapd` or `gadget` are skipped.
   - **Removal:** `snap remove --purge <name>`, as a PrivilegedCommand.
   - **Leftovers:** `~/snap/<name>` is already found by the M2 candidates.
   - **Post-check:** `snap list <name>` fails, meaning the snap is gone.
7. **apt and dnf system categories.**
   - **`pkg-cache`:**
     - Debian: the size is the sum of `/var/cache/apt/archives/*.deb`, and the command is `apt-get clean`.
     - Fedora: the size is the sum of `*.rpm` under `/var/cache/libdnf5` (dnf5) or `/var/cache/dnf` (dnf4), and the command is `dnf clean packages`.
   - **`orphan-pkgs`:**
     - Debian: simulate `apt-get -s autoremove --purge` and execute `apt-get purge -y <exact list>`.
     - Fedora: simulate `dnf autoremove --assumeno` and execute `dnf remove -y --setopt=clean_requirements_on_remove=False <exact list>`.
     - Never `autoremove` as root. The exact-list rule is the same as for pacman.
   - Categories with no matching package manager report `unavailable`.
8. **Running as root (uid 0).**
   - `detect_escalation` returns `Escalation::AlreadyRoot`, which runs the batch as `/bin/sh -s` with the script on stdin. No escalation tool is involved.
   - Every user-level rule still applies to the root user's own home.
   - System-tree deletes keep every re-check, including the namespace guard, which then trivially passes.
9. **Distro matrix.**
   - `scripts/distro-check.sh` runs inside rootless podman containers. It covers these images:
     - `docker.io/library/archlinux:latest`
     - `docker.io/library/debian:trixie`
     - `docker.io/library/ubuntu:latest`
     - `registry.fedoraproject.org/fedora:latest`
   - In each container it:
     - builds a debug crumbs from the bind-mounted source, with the target in a named volume per image;
     - runs `cargo test --workspace`;
     - runs the read-only smoke tests `crumbs doctor --json`, `crumbs apps list --json` and `crumbs scan --json`;
     - runs an end-to-end uninstall as the container's root: install a small package (Debian/Ubuntu: `sl`, Fedora: `sl`, Arch: `sl`), run `crumbs uninstall sl --yes --json`, then assert the package is gone.
   - A disposable container is the only place where a real, non-dry-run removal is allowed.
   - `.github/workflows/ci.yml` runs the same script per image as a job matrix. It is not runnable here, because the repo has no remote.

## Global Constraints

- All M1–M4c global constraints still apply. See the M4c plan, "Global Constraints".
- **No new dependencies.**
- **On the host, the dev machine (Manjaro):**
  - never run `uninstall`, `clean`, `orphans --clean` or `restore` without `--dry-run`;
  - never call MCP `apply`;
  - never escalate;
  - never run `cargo test --release`.
- **Inside the disposable podman containers** started by `scripts/distro-check.sh`, real removals as the container's root are allowed. They are the point of the end-to-end test. Never bind-mount host system paths, only the repo, read-only, plus a cargo volume. Never use `--privileged`.
- **Package names** for apt, dnf and snap must match `^[A-Za-z0-9][A-Za-z0-9.+_-]*$` before any command; a name that fails gets `RemovalError::InvalidName`.
- **Fixtures:** every parser is tested with captured real output in `crates/crumbs-core/tests/fixtures/<tool>/`, captured from the containers:
  - `dpkg/status`, `apt-get -s purge`, `apt-get -s autoremove`;
  - `rpm -qa` queries, dnf4 and dnf5 `remove --assumeno`, `autoremove --assumeno`;
  - `snap list`.

  The capture commands go in `tests/fixtures/README.md`.
- **Disk space:** `/tmp` is a small tmpfs, so use `~/.cache` for scratch and podman volumes.

## Review Focus

1. **Simulation and execution disagree.** For example, a dependency appears between the preview and the run. The execution must never remove more than the preview listed. Pinned by `apt_exact_list_never_autoremoves` (Task 5) and `dnf_exact_list_disables_clean_requirements` (Task 6).
2. **Localised package-manager output.** For example `LANG=de_DE`. Parsers must see English output, because every command runs with `LC_ALL=C` from the SystemRunner. Pinned by `apt_simulation_is_run_with_c_locale` (Task 5).
3. **A dpkg package in `rc` (config-files) state, or a half-installed package.** It is not listed as installed, and its `.list` still counts toward ownership. Pinned by `dpkg_rc_packages_not_listed_but_owned` (Task 2).
4. **dnf4 versus dnf5 output, or an unknown future format.** Both parse, and anything else refuses. Pinned by `dnf_unknown_table_refuses` (Task 6).
5. **Running crumbs as root.** The batch runs without any escalation tool, user-level rules hold for root's home, and nothing else changes. Pinned by `already_root_runs_batch_with_sh` (Task 7).

---

### Task 1: Package database dispatch

**Files:**
- Modify: `crates/crumbs-core/src/ownership.rs`, `inventory.rs`, `leftovers/system.rs`, `removal/unmanaged.rs`, `executor/mod.rs` (`Rechecks`).

**Interfaces:**
- Produces:
  - `pub enum PackageDb { Pacman, Dpkg, Rpm }`.
  - `pub fn detect_package_db(env: &Env) -> Option<PackageDb>`.
  - `pub fn load_package_db(env: &Env, runner: &dyn CommandRunner) -> Result<Option<OwnedIndex>, CoreError>`. It dispatches to `load_pacman`, `load_dpkg` (Task 2) and `load_rpm` (Task 3). Until those exist, the dpkg and rpm arms return `Ok(None)`.
  - Every caller of `load_pacman` (the M4b executor re-check and the CLI) switches to `load_package_db`.
  - The error message `"the package database could not be read"` is unchanged.
- Tests:
  - `detect_prefers_pacman_then_dpkg_then_rpm`;
  - `existing_pacman_behaviour_unchanged`: every M4b test passes unchanged.

- [ ] **Step 1: Write the failing tests.**
- [ ] **Step 2: Run.** Expected: FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** `cargo test --workspace`. Expected: PASS.
- [ ] **Step 5: Commit** `refactor(core): package database dispatch`.

### Task 2: dpkg source

**Files:**
- Create: `crates/crumbs-core/src/sources/dpkg.rs`, `tests/fixtures/dpkg/`.
- Modify: `model.rs` (`SourceKind::Dpkg`, serialised `dpkg`), `sources/mod.rs`, `inventory.rs` (`default_sources`), `ownership.rs` (`load_dpkg`).

**Interfaces:**
- Produces:
  - `DpkgSource`, with the same listing contract as `PacmanSource`:
    - GUI apps by default are packages that own a `.desktop` file with `NoDisplay` not true;
    - `include_all` lists every package;
    - owned files are in `SourceListing.owned_paths` and `owned_dirs`.
  - `ownership::load_dpkg(env) -> Result<Option<OwnedIndex>, CoreError>`.
  - App ids are `dpkg:<name>`.
- Tests:
  - `dpkg_lists_gui_packages_from_status`, using a fixture status with firefox-esr, coreutils and an `rc` package;
  - `dpkg_rc_packages_not_listed_but_owned`;
  - `dpkg_multiarch_list_files_are_read` (`libfoo:amd64.list`);
  - `dpkg_install_size_is_kib_times_1024`;
  - `dpkg_unreadable_list_makes_index_incomplete`;
  - `load_package_db_on_debian_fixture_owns_usr_bin_ls`.

- [ ] **Step 1: Capture the fixtures.**
  - In `podman run --rm docker.io/library/debian:trixie`, copy `/var/lib/dpkg/status` and a few `info/*.list` files.
  - Trim them to the packages the tests need.
  - Record the commands in `tests/fixtures/README.md`.
- [ ] **Step 2: Write the failing tests.**
- [ ] **Step 3: Run.** Expected: FAIL.
- [ ] **Step 4: Implement.**
- [ ] **Step 5: Run** `cargo test --workspace`. Expected: PASS.
- [ ] **Step 6: Commit** `feat(core): dpkg source`.

### Task 3: rpm source

**Files:**
- Create: `crates/crumbs-core/src/sources/rpm.rs`, `tests/fixtures/rpm/`.
- Modify: `model.rs` (`SourceKind::Rpm`), `sources/mod.rs`, `inventory.rs`, `ownership.rs` (`load_rpm`).

**Interfaces:**
- Produces:
  - `RpmSource`, using the Decision 3 queries through the `CommandRunner`, with the same listing contract as dpkg.
  - `ownership::load_rpm(env, runner)`.
  - App ids are `rpm:<name>`.
- Tests:
  - `rpm_lists_packages_from_query`, using fixture query output;
  - `rpm_epoch_zero_omitted_from_version` (`0:1.2-3.fc41.x86_64` is shown as `1.2-3.fc41`);
  - `rpm_query_failure_is_source_error_and_index_incomplete`;
  - `rpm_filenames_include_dirs`.

- [ ] **Step 1: Capture the fixtures.** In `registry.fedoraproject.org/fedora:latest`, run both queries and trim the output.
- [ ] **Step 2: Write the failing tests.**
- [ ] **Step 3: Run.** Expected: FAIL.
- [ ] **Step 4: Implement.**
- [ ] **Step 5: Run** `cargo test --workspace`. Expected: PASS.
- [ ] **Step 6: Commit** `feat(core): rpm source`.

### Task 4: Snap source and removal

**Files:**
- Create: `sources/snap.rs`, `removal/snap.rs`, `tests/fixtures/snap/`.
- Modify: `model.rs` (`SourceKind::Snap`), `removal/mod.rs` (`plan_removal`, `still_installed`).

**Interfaces:**
- Produces:
  - `SnapSource`, following Decision 6. App ids are `snap:<name>`.
  - `removal::snap::plan(app) -> RemovalPlan`: a single PrivilegedCommand `snap remove --purge <name>`.
  - `still_installed(Snap, names)` via `snap list <name>`.
- Tests:
  - `snap_list_skips_bases_and_snapd`;
  - `snap_removal_is_purge_privileged`;
  - `snap_name_validated`;
  - `snap_post_check_gone_when_list_fails`.

  The fixture is a hand-written `snap list` output that follows the documented column layout, because snap needs systemd and is not available in the containers. Say so in the fixtures README.

- [ ] **Steps 1–5:** write the failing tests, run them (FAIL), implement, run `cargo test --workspace` (PASS), then commit `feat(core): snap source and removal`.

### Task 5: apt removal

**Files:**
- Create: `crates/crumbs-core/src/removal/apt.rs`, `tests/fixtures/apt/`.
- Modify: `removal/mod.rs`.

**Interfaces:**
- Consumes: `PackageRemove { source: Dpkg, names, versions, program: "apt-get", args }`, the M4 action.
- Produces:
  - `removal::apt::plan(env, runner, app) -> Result<RemovalPlan, RemovalError>`, following Decision 4.
  - `still_installed(Dpkg, names)` by re-reading `dpkg/status`.
  - The refusal text is apt's `E:` lines.
  - The busy message is `apt is busy (another package manager is running)`.
  - History `package-remove` entries gain `source` (`pacman`, `dpkg`, `rpm` or `snap`; serialised, absent means `pacman` for old entries).
  - The restore hint uses the source:
    - dpkg: `sudo apt-get install <names>`, with `--mark-auto` for dependencies via `apt-mark auto`;
    - rpm: `sudo dnf install <names>`;
    - snap: `sudo snap install <names>`.

    This was an M4 ruling ("M5 must add it for apt/dnf").
- Tests:
  - `restore_hint_names_the_package_manager`, covering all four sources;
  - `apt_simulation_lists_purged_and_dependents`;
  - `apt_exact_list_never_autoremoves`: the args are exactly `purge -y --no-install-recommends <names>`, with no `autoremove` and no `--auto-remove`;
  - `apt_simulation_with_install_refuses`;
  - `apt_error_lines_become_refusal`;
  - `apt_lock_message_is_busy`;
  - `apt_simulation_is_run_with_c_locale`: the FakeRunner records the environment, or the SystemRunner contract is asserted;
  - `apt_post_check_rc_state_counts_as_still_installed`.

- [ ] **Step 1: Capture the fixtures.** In debian:trixie as root, run `apt-get update`, install `sl` and `cowsay`, then run `apt-get -s purge sl` and `apt-get -s purge perl-base`. The second shows errors and essential packages.
- [ ] **Steps 2–6:** write the failing tests, run them (FAIL), implement, run `cargo test --workspace` (PASS), then commit `feat(core): apt removal`.

### Task 6: dnf removal

**Files:**
- Create: `crates/crumbs-core/src/removal/dnf.rs`, `tests/fixtures/dnf/`.
- Modify: `removal/mod.rs`.

**Interfaces:**
- Produces:
  - `removal::dnf::plan(env, runner, app)`, following Decision 5.
  - `still_installed(Rpm, names)` via `rpm -q`.
- Tests:
  - `dnf5_table_parsed`;
  - `dnf4_table_parsed`, using a hand-made fixture from dnf4's documented format if no dnf4 image is available. Note this in the fixtures README.
  - `dnf_exact_list_disables_clean_requirements`;
  - `dnf_unknown_table_refuses`;
  - `dnf_nonzero_assumeno_is_expected`;
  - `rpm_post_check`.

- [ ] **Step 1: Capture the fixtures.** In fedora:latest, install `sl`, then run `dnf remove --assumeno sl` and `dnf autoremove --assumeno`.
- [ ] **Steps 2–6:** write the failing tests, run them (FAIL), implement, run `cargo test --workspace` (PASS), then commit `feat(core): dnf removal`.

### Task 7: apt and dnf system categories, and running as root

**Files:**
- Modify:
  - `bloat/system/{pkg_cache.rs,orphans.rs}`;
  - `privilege/{mod.rs,batch.rs}` (`Escalation::AlreadyRoot`);
  - `crates/crumbs-cli/src/cmd/act/privilege.rs`.

**Interfaces:**
- Produces:
  - Per Decision 7, `pkg-cache` and `orphan-pkgs` dispatch on `detect_package_db`. A variant that needs a tool which is absent reports `unavailable (<tool> not installed)`.
  - Per Decision 8, an `AlreadyRoot` escalation runs the script through `/bin/sh -s`, from `resolve_trusted("sh")`, with stdin as the script and stdout captured. On a fake root, the existing test-runner gating applies first.
  - `doctor` prints `privileged actions: running as root`.
- Tests:
  - `apt_pkg_cache_size_from_archives`;
  - `dnf5_pkg_cache_size`;
  - `apt_orphans_exact_purge_list`;
  - `dnf_orphans_exact_remove_list`;
  - `already_root_runs_batch_with_sh`, using a fake root with `uid = 0` via a test-only `Env` setter;
  - `already_root_is_never_chosen_for_non_root`.

- [ ] **Steps 1–5:** write the failing tests, run them (FAIL), implement, run `cargo test --workspace` (PASS), then commit `feat(core): apt and dnf system categories; run as root`.

### Task 8: Distro matrix

**Files:**
- Create: `scripts/distro-check.sh`, `.github/workflows/ci.yml`.
- Modify: `crates/crumbs-cli/tests/cli.rs` only if needed for container-specific skips.

**Interfaces:**
- `scripts/distro-check.sh [image…]` takes the Decision 9 image list by default. For each image:
  1. Install the toolchain with the distro's package manager: `rustup` via `curl`, or the distro's `cargo` if it meets `rust-version = "1.99"`. Use the rustup minimal profile pinned to the workspace's toolchain.
  2. Build crumbs.
  3. Run `cargo test --workspace`.
  4. Run the three smoke commands and validate that the JSON parses with `crumbs … | python3 -m json.tool`, or with `jq` if present.
  5. Run the end-to-end uninstall of `sl`.
  6. Print `PASS <image>` or `FAIL <image>: <step>`.
  - Exit status is non-zero if any image fails.
- Each image's cargo target and registry go in named podman volumes (`crumbs-target-<distro>`, `crumbs-cargo-<distro>`).
- The repo is mounted read-only at `/src`, with `CARGO_TARGET_DIR` pointing into the volume.
- `ci.yml` has a job matrix over the same images, `container:` per job, running the same script with `--inside`.

- [ ] **Step 1: Write the script and workflow.**
- [ ] **Step 2: Run** `scripts/distro-check.sh` on the dev machine. It pulls `ubuntu:latest`; the other three images are already present. Expected: `PASS` for all four. Paste the summary into the report.
- [ ] **Step 3: Fix** any distro-specific failures in the code they reveal, each with a test.
- [ ] **Step 4: Commit** `ci: podman distro matrix`.

### Task 9: Docs

**Files:**
- Modify:
  - `README.md`: supported distros and sources, what needs which tool, and running the distro check;
  - `docs/json-schema.md`: the new `source` values `dpkg`, `rpm` and `snap`, plus `package-remove` with `source` and the apt/dnf args;
  - spec §13: M5 done.
- Create: `docs/superpowers/notes/2026-11-09-m5-review-followups.md`, covering zypper, rpm `-e`, `dpkg -P` when apt-get is missing, a dnf4 image, Snap in containers, and anything the ledger parks.

- [ ] **Step 1: Write the docs.**
- [ ] **Step 2: Cross-check them** against the serde values and the CLI output.
- [ ] **Step 3: Commit** `docs: cross-distro support`.
