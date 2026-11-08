# crumBS Milestone 1 (Foundation) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A working `crumbs` CLI that lists installed GUI apps from pacman, Flatpak, AppImage and unmanaged `.desktop` sources (`apps list`, `apps show`) and reports environment health (`doctor`), with human and JSON output.

**Architecture:** Cargo workspace with `crumbs-core` (all logic, no terminal I/O) and `crumbs-cli` (clap, rendering). Every filesystem path goes through `Env`, so tests run against fake trees under the system temp dir. External commands go through a `CommandRunner` trait and are only resolved from trusted system dirs. A `test-support` feature in core exposes a fixture-tree builder that the CLI tests reuse.

**Tech Stack:** Rust (edition 2024, stable 1.99), clap 4 (derive), serde + serde_json, thiserror 2, anyhow, anstream + anstyle, tempfile, assert_cmd, predicates, insta (json feature).

**Spec:** `docs/superpowers/specs/2026-11-08-crumbs-design.md` (sections 2, 3, 9, 11, and milestone 1 of section 13).

## Global Constraints

- Workspace members: `crates/crumbs-core` (lib, crate name `crumbs_core`) and `crates/crumbs-cli` (binary name `crumbs`).
- Edition 2024; `rust-version = "1.99"`.
- Add dependencies with `cargo add` so versions are current; no others without updating this plan.
- `crumbs-core` never prints and never reads stdin. It uses `thiserror` errors; `anyhow` only appears in `crumbs-cli`.
- No `unwrap()`/`expect()` in non-test library code.
- `cargo clippy --workspace --all-targets -- -D warnings` and `cargo fmt --check` pass at every commit.
- Every system path such as `/var/lib/pacman/local` is built as `env.sys("/var/lib/pacman/local")`. Never hard-code an absolute path in a `Path::new`.
- External programs are resolved only from `/usr/bin`, `/usr/sbin`, `/bin`, `/sbin` (each joined to `env.root`), never from `$PATH`.
- JSON output is one document per invocation: `{"schema_version": 1, "command": "<cmd>", "data": {...}, "errors": [...]}`.
- Exit codes: 0 success, 1 error, 2 usage error or ambiguous app, 3 partial failure, 4 aborted by user, 5 privilege unavailable.
- Sizes for humans are IEC (`KiB`, `MiB`, `GiB`) with one decimal place; bytes under 1024 print as `N B`.
- Colour only when stdout is a TTY and `NO_COLOR` is unset (anstream handles this).
- Out of scope for M1: leftovers, any deletion, config file, dpkg/rpm/snap, `--show-low`. The `AppSource` trait gains `removal_plan` in M2.

## Review Focus

1. **Malformed pacman entries** (missing `%SIZE%`, missing `files`, non-UTF-8 path bytes): skip or degrade that one package and record a `SourceError`. Never fail the whole listing. Pinned in Task 5.
2. **Messy desktop files** (BOM, `[Desktop Action x]` groups after the main group, `Name[de]=` localized keys, CRLF line endings, duplicate keys): only the main `[Desktop Entry]` group counts, the first value wins, and localized keys are ignored. Pinned in Task 4.
3. **Exec lines in the wild** (`env FOO=1 /opt/x/x %U`, `"/opt/My App/app" %f`, `flatpak run org.x.Y`, `sh -c "…"`, `appofholding run gutcheck %U`): the exec program is the first non-`env`-assignment token, with quotes removed and field codes dropped. Pinned in Task 4.
4. **Hostile AppImage dirs** (broken symlinks, unreadable files, subdirectories, zero-byte files, a missing dir): skipped silently; only valid AppImages are listed. Pinned in Task 7.
5. **Ambiguous `apps show` queries** (two apps with display name "Firefox" from pacman and Flatpak): exit 2 and list both candidates with their source-qualified IDs. Pinned in Task 10.

---

## File Structure

```
Cargo.toml                                # workspace
crates/crumbs-core/Cargo.toml             # features: test-support
crates/crumbs-core/src/lib.rs
crates/crumbs-core/src/env.rs             # Env, Distro
crates/crumbs-core/src/error.rs           # CoreError, SourceError
crates/crumbs-core/src/model.rs           # App, SourceKind, Installation
crates/crumbs-core/src/command.rs         # CommandRunner, SystemRunner, CmdOutput, resolve_trusted
crates/crumbs-core/src/desktop.rs         # DesktopEntry parse + discovery, exec parsing
crates/crumbs-core/src/sources/mod.rs     # AppSource trait, SourceListing, ListOptions
crates/crumbs-core/src/sources/pacman.rs
crates/crumbs-core/src/sources/flatpak.rs
crates/crumbs-core/src/sources/appimage.rs
crates/crumbs-core/src/sources/unmanaged.rs
crates/crumbs-core/src/inventory.rs       # discover(), Inventory, find_app()
crates/crumbs-core/src/privilege.rs       # Escalation detection only (M1)
crates/crumbs-core/src/doctor.rs          # DoctorReport
crates/crumbs-core/src/testing.rs         # FixtureTree, FakeRunner (cfg feature test-support)
crates/crumbs-cli/Cargo.toml
crates/crumbs-cli/src/main.rs             # clap, dispatch, exit codes
crates/crumbs-cli/src/output.rs           # JSON envelope, fmt_size, table rendering
crates/crumbs-cli/src/cmd/apps.rs
crates/crumbs-cli/src/cmd/doctor.rs
crates/crumbs-cli/tests/cli.rs            # assert_cmd + insta
```

Unit tests live in `#[cfg(test)] mod tests` inside each core file.

---

### Task 1: Workspace, Env and test fixtures

**Files:**
- Create: `Cargo.toml`, `.gitignore` (`/target`), `crates/crumbs-core/{Cargo.toml,src/lib.rs,src/env.rs,src/error.rs,src/testing.rs}`, `crates/crumbs-cli/{Cargo.toml,src/main.rs}` (main prints nothing yet)

**Interfaces:**
- Produces:
  - `pub struct Env { pub root: PathBuf, pub home: PathBuf, pub config_home: PathBuf, pub cache_home: PathBuf, pub data_home: PathBuf, pub state_home: PathBuf, pub data_dirs: Vec<PathBuf>, pub uid: u32, pub distro: Distro }`
  - `Env::from_system() -> Result<Env, CoreError>`: root `/`. XDG vars are used when set and absolute, otherwise spec defaults. `data_dirs` comes from `XDG_DATA_DIRS`, or `/usr/local/share:/usr/share` if unset. If missing, append `{data_home}/flatpak/exports/share` and `/var/lib/flatpak/exports/share`. uid comes from `rustix::process::getuid()` (`cargo add rustix --features process`).
  - `Env::fake(root: &Path) -> Result<Env, CoreError>`: returns `CoreError::UnsafeRoot` unless `root` canonicalizes to a path under `std::env::temp_dir()` canonicalized. home = `root/home/user`, XDG dirs are the defaults under that home, data_dirs = `[root/usr/local/share, root/usr/share]`, uid = current uid.
  - `Env::sys(&self, abs: &str) -> PathBuf`: joins `abs` minus its leading `/` onto `root`.
  - `Env::unsys(&self, p: &Path) -> PathBuf`: the inverse; strips `root` and returns the absolute in-system path, used for display and JSON.
  - `pub struct Distro { pub id: String, pub id_like: Vec<String>, pub name: String }`, parsed from `env.sys("/etc/os-release")` (quotes stripped). Missing file gives `id="unknown"`.
  - `pub enum CoreError { Io { path: PathBuf, source: io::Error }, UnsafeRoot(PathBuf), Command { program: String, message: String }, Parse { what: String, message: String } }`
  - (feature `test-support`) `pub struct FixtureTree { pub dir: tempfile::TempDir }` with `new()`, `env() -> Env`, `write(rel: &str, contents: &[u8]) -> PathBuf` (creates parents; `rel` is an in-system absolute path like `/home/user/x`), `symlink(rel, target)`, `mkdir(rel)`, `os_release(id, id_like, name)`, `pacman_pkg(name, version, size: Option<u64>, files: &[&str])`, `desktop(rel_path, contents)`, `appimage(rel_path)` (writes `\x7fELF` + 4 zero bytes + `AI\x02` + padding).

- [ ] **Step 1: Write failing tests in `env.rs`**
  - `fake_rejects_non_temp_root`: `Env::fake(Path::new("/"))` is `Err(CoreError::UnsafeRoot(_))`.
  - `sys_joins_under_root`: with fake root R, `env.sys("/var/lib/pacman/local") == R.join("var/lib/pacman/local")` and `env.unsys(&that) == Path::new("/var/lib/pacman/local")`.
  - `xdg_defaults_under_home`: `env.cache_home == env.home.join(".cache")`, and likewise `.config`, `.local/share`, `.local/state`.
  - `distro_parsed`: after `os_release("manjaro", "arch", "Manjaro Linux")`, `env.distro.id_like == ["arch"]`.
  - `distro_missing_is_unknown`.
- [ ] **Step 2: Run** `cargo test -p crumbs-core --features test-support env::` and expect compile failure or FAIL.
- [ ] **Step 3: Implement** `Env`, `Distro`, the errors and `FixtureTree`.
- [ ] **Step 4: Run** the same command, plus clippy and fmt. Expect PASS.
- [ ] **Step 5: Commit** `feat(core): workspace, Env and fixture tree`

---

### Task 2: Model types

**Files:** Create `crates/crumbs-core/src/model.rs`

**Interfaces:**
- Produces (all `Serialize, Debug, Clone, PartialEq`; enums `#[serde(rename_all = "kebab-case")]`):
  - `pub enum SourceKind { Pacman, Flatpak, AppImage, Unmanaged }` with `as_str()` returning `pacman|flatpak|appimage|unmanaged` and `FromStr` for clap.
  - `pub enum Installation { User, System }`
  - `pub struct App { pub id: String, pub name: String, pub display_name: String, pub version: Option<String>, pub description: Option<String>, pub source: SourceKind, pub installation: Installation, pub install_size: Option<u64>, pub location: Option<PathBuf>, pub desktop_ids: Vec<String>, pub rdns_id: Option<String>, pub exec_names: Vec<String>, pub wm_classes: Vec<String>, pub has_gui: bool }`
  - `id` is source-qualified: `pacman:firefox`, `flatpak:org.mozilla.firefox`, `appimage:GutCheck-0.19.0-x86_64.AppImage`, `unmanaged:<desktop-id>`.
  - `App::new(source: SourceKind, name: &str) -> App` sets `id` and `display_name = name` and leaves everything else empty, so sources fill in fields without long literals.
  - `pub struct SourceError { pub source: SourceKind, pub message: String }` (Serialize). It lives in `error.rs` and is populated from Task 5.
  - Paths in `App` are in-system (`env.unsys` applied), never fixture-root paths.

- [ ] **Step 1: Failing tests:**
  - `app_id_is_source_qualified`
  - `source_kind_round_trips_from_str`
  - `app_serializes_source_names`: JSON `source` values are exactly `pacman`, `flatpak`, `appimage` and `unmanaged`. The `AppImage` variant needs `#[serde(rename = "appimage")]`, because kebab-case would produce `app-image`.
- [ ] **Step 2: Run** `cargo test -p crumbs-core model::` and expect FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** and expect PASS.
- [ ] **Step 5: Commit** `feat(core): app model`

---

### Task 3: Command runner and trusted resolution

**Files:** Create `crates/crumbs-core/src/command.rs`, and extend `testing.rs` with `FakeRunner`.

**Interfaces:**
- Produces:
  - `pub struct CmdOutput { pub status: i32, pub stdout: String, pub stderr: String }` (stdout/stderr decoded with `String::from_utf8_lossy`; a signal-terminated process gives status -1)
  - `pub trait CommandRunner: Send + Sync { fn run(&self, program: &Path, args: &[&str]) -> Result<CmdOutput, CoreError>; }`
  - `pub struct SystemRunner;` uses `std::process::Command` with `LC_ALL=C` and stdin set to null.
  - `pub const TRUSTED_DIRS: [&str; 4] = ["/usr/bin", "/usr/sbin", "/bin", "/sbin"];`
  - `pub fn resolve_trusted(env: &Env, name: &str) -> Option<PathBuf>` returns the first `env.sys(dir)/name` that is a regular file with any execute bit set. It returns the **root-joined** path, which is what gets executed. A `name` containing `/` gives `None`.
  - (test-support) `pub struct FakeRunner` with `FakeRunner::new()` and `.on(program_basename: &str, args: &[&str], out: CmdOutput) -> Self`. Unregistered calls return `CoreError::Command`. `.calls() -> Vec<String>` records every call as `"basename arg1 arg2"`.

- [ ] **Step 1: Failing tests:**
  - `resolves_from_usr_bin`
  - `ignores_non_executable`
  - `ignores_home_local_bin`: put an executable `flatpak` only in `/home/user/.local/bin` and expect `None`.
  - `rejects_name_with_slash`
  - `fake_runner_returns_registered_output`
  - `fake_runner_errors_on_unregistered`
- [ ] **Step 2: Run** `cargo test -p crumbs-core --features test-support command::` and expect FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** and expect PASS.
- [ ] **Step 5: Commit** `feat(core): command runner with trusted path resolution`

---

### Task 4: Desktop entry parsing and discovery

**Files:** Create `crates/crumbs-core/src/desktop.rs`

**Interfaces:**
- Produces:
  - `pub struct DesktopEntry { pub id: String, pub path: PathBuf /* root-joined */, pub name: Option<String>, pub exec: Option<String>, pub exec_program: Option<String>, pub exec_args: Vec<String>, pub try_exec: Option<String>, pub wm_class: Option<String>, pub no_display: bool, pub hidden: bool, pub is_application: bool, pub appimage_version: Option<String>, pub is_appimage_entry: bool }`
  - `pub fn parse_desktop(id: &str, path: &Path, bytes: &[u8]) -> DesktopEntry`. Decode lossily and strip a UTF-8 BOM. Accept CRLF. Only the `[Desktop Entry]` group counts. Skip comments and blank lines. For a duplicate key the first value wins. Ignore keys containing `[`. `is_application` = `Type=Application`. `is_appimage_entry` = any key starting with `X-AppImage`. `appimage_version` comes from `X-AppImage-Version`.
  - `pub fn split_exec(exec: &str) -> Vec<String>`: tokenize per the Desktop Entry spec, honouring double quotes and backslash escapes inside them, and drop the field codes `%f %F %u %U %d %D %n %N %i %c %k %v %m`. `%%` becomes `%`.
  - `exec_program` = the first token after skipping a leading `env` and any following `NAME=value` tokens. `exec_args` = the remaining tokens.
  - `pub fn discover_desktop_entries(env: &Env) -> Vec<DesktopEntry>`: search `{data_home}/applications`, then each `{data_dir}/applications`, recursively. The desktop ID is the relative path with `/` replaced by `-`. The first occurrence of an ID wins (user entries shadow system ones). Unreadable files are skipped.
  - `pub fn exec_basename(e: &DesktopEntry) -> Option<String>` returns the file name of `exec_program`.

- [ ] **Step 1: Failing tests** (from Review Focus #2 and #3):
  - `parses_main_group_only`: `[Desktop Action new]` with `Exec=other` after the main group is ignored.
  - `ignores_localized_keys`: `Name[de]=Feuerfuchs` before `Name=Firefox` gives `name == Some("Firefox")`.
  - `handles_bom_and_crlf`
  - `first_duplicate_key_wins`
  - `exec_env_prefix`: `env FOO=1 BAR=2 /opt/x/x %U` gives program `/opt/x/x` and empty args.
  - `exec_quoted_path`: `"/opt/My App/app" %f` gives `/opt/My App/app`.
  - `exec_flatpak_run`: `flatpak run org.x.Y` gives program `flatpak` and args `["run","org.x.Y"]`.
  - `exec_percent_escape`: `foo 100%%` gives args `["100%"]`.
  - `appimage_keys_detected`: `X-AppImage-Version=0.19.0` sets both AppImage fields.
  - `user_entry_shadows_system`: the same `foo.desktop` in `data_home/applications` and `/usr/share/applications` yields one entry, the user one.
  - `subdir_id_uses_dash`: `/usr/share/applications/kde4/kate.desktop` gets the ID `kde4-kate.desktop`.
- [ ] **Step 2: Run** `cargo test -p crumbs-core --features test-support desktop::` and expect FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** and expect PASS.
- [ ] **Step 5: Commit** `feat(core): desktop entry parser and discovery`

---

### Task 5: AppSource trait and pacman source

**Files:** Create `crates/crumbs-core/src/sources/mod.rs` and `crates/crumbs-core/src/sources/pacman.rs`

**Interfaces:**
- Consumes: `Env`, `App`, `DesktopEntry`, `discover_desktop_entries`, `CommandRunner`.
- Produces:
  - `pub struct ListOptions { pub include_all: bool }` (Default: false).
  - `pub struct SourceContext<'a> { pub env: &'a Env, pub runner: &'a dyn CommandRunner, pub desktop: &'a [DesktopEntry], pub opts: &'a ListOptions }`
  - `pub struct SourceListing { pub apps: Vec<App>, pub owned_paths: HashSet<PathBuf> /* in-system absolute */, pub claimed_desktop_ids: HashSet<String>, pub errors: Vec<SourceError> }` (Default).
  - `pub trait AppSource: Send + Sync { fn kind(&self) -> SourceKind; fn available(&self, env: &Env) -> bool; fn is_package_db(&self) -> bool; fn list(&self, cx: &SourceContext) -> Result<SourceListing, CoreError>; }`
  - `pub struct PacmanSource;` It is available when `env.sys("/var/lib/pacman/local")` is a directory, and `is_package_db() == true`.
- Behaviour:
  - Read each `local/<dir>/desc`, taking `%NAME%`, `%VERSION%`, `%DESC%` and `%SIZE%`. The value is the line(s) after the header up to the next blank line.
  - Read `local/<dir>/files` (the `%FILES%` section). Entries are relative with no leading `/`. Prefix them with `/`, drop trailing-`/` directory entries, and add them to `owned_paths`.
  - Owned files under `/usr/share/applications/*.desktop` link the package to entries by desktop ID.
  - `has_gui` = it owns at least one linked entry that is an application, not `no_display` and not `hidden`. Fill in `desktop_ids`, `exec_names` (via `exec_basename`) and `wm_classes`. `display_name` = the first linked entry's `name`, falling back to the package name.
  - With `include_all == false`, only packages with `has_gui` go into `apps`. `owned_paths` always covers every package.
  - `installation = System`. `location = None`.
  - Add every linked desktop ID to `claimed_desktop_ids`.
  - A dir without `desc` or without `%NAME%` adds a `SourceError` and is skipped. A missing `files` file means no owned paths and no GUI. A missing `%SIZE%` gives `install_size = None`. Non-UTF-8 lines in `files` are decoded lossily.

- [ ] **Step 1: Failing tests:**
  - `lists_gui_package`: firefox owns `usr/bin/firefox` and `usr/share/applications/firefox.desktop`. Expect one app with id `pacman:firefox`, display name taken from the entry, `install_size == Some(316982060)`, and `exec_names == ["firefox"]`.
  - `hides_cli_package_unless_all`: `coreutils`, which has no desktop file, is absent by default and present with `include_all`.
  - `no_display_entry_is_not_gui`
  - `owned_paths_cover_all_packages`: `/usr/bin/ls` from coreutils is in `owned_paths` even without `--all`.
  - `malformed_entry_recorded_not_fatal` (Review Focus #1): one dir has no `desc`, one has a `desc` without `%SIZE%`, and one has non-UTF-8 bytes in `files`. The listing still succeeds with one `SourceError`, and the size-less package has `install_size == None`.
  - `unavailable_without_db`
- [ ] **Step 2: Run** `cargo test -p crumbs-core --features test-support sources::pacman` and expect FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** and expect PASS.
- [ ] **Step 5: Commit** `feat(core): AppSource trait and pacman source`

---

### Task 6: Flatpak source

**Files:** Create `crates/crumbs-core/src/sources/flatpak.rs`

**Interfaces:**
- Consumes: `AppSource`, `SourceContext`, `resolve_trusted`.
- Produces:
  - `pub struct FlatpakSource;` It is available when `resolve_trusted(env, "flatpak")` is `Some`, and `is_package_db() == false`.
  - `pub fn parse_si_size(s: &str) -> Option<u64>` parses decimal SI sizes such as `34.4 MB`, `593.5 MB`, `1.2 GB`, `900 bytes` and `12.0 kB` (1 kB = 1000 B). The match is case-insensitive.
- Behaviour:
  - Run `flatpak list --app --columns=application,name,version,installation,size` and parse it as TSV.
  - `id = flatpak:<application>`, `name = rdns_id = application`, `display_name` = the name column, `version` (an empty string becomes `None`), `installation` from `user`/`system`, `has_gui = true`, `desktop_ids = ["<application>.desktop"]`, `install_size` via `parse_si_size`.
  - `location` = `{data_home}/flatpak/app/<id>` for user installs and `/var/lib/flatpak/app/<id>` for system installs.
  - Add the desktop ID to `claimed_desktop_ids`.
  - A non-zero exit returns `Ok` with a `SourceError` holding the trimmed stderr and no apps.

- [ ] **Step 1: Failing tests** (using `FakeRunner` and a fixture executable at `/usr/bin/flatpak`):
  - `parses_flatpak_list`: use the three real rows `app.xemu.xemu\txemu\t0.8.136\tuser\t34.4 MB`, `com.dosbox_x.DOSBox-X\tDOSBox-X\t2026.10.01\tsystem\t51.4 MB` and `com.obsproject.Studio\tOBS Studio\t33.0.0\tsystem\t593.5 MB`. Expect three apps; OBS has `install_size == Some(593_500_000)` and `installation == System`.
  - `si_sizes`: `parse_si_size("12.0 kB") == Some(12_000)`, `"900 bytes"` gives `Some(900)`, and `"?"` gives `None`.
  - `nonzero_exit_is_source_error`
  - `unavailable_when_not_in_trusted_dirs`
- [ ] **Step 2: Run** `cargo test -p crumbs-core --features test-support sources::flatpak` and expect FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** and expect PASS.
- [ ] **Step 5: Commit** `feat(core): flatpak source`

---

### Task 7: AppImage source

**Files:** Create `crates/crumbs-core/src/sources/appimage.rs`

**Interfaces:**
- Produces:
  - `pub struct AppImageSource;` It is always available, and `is_package_db() == false`.
  - `pub fn is_appimage(bytes: &[u8]) -> bool`: true when bytes 0..4 are `\x7fELF` and bytes 8..11 are `AI\x02`.
  - `pub fn split_appimage_name(file_name: &str) -> (String, Option<String>)`.
  - `pub fn normalize_name(s: &str) -> String`: lowercase with `-`, `_`, `.` and spaces removed. Later tasks reuse it.
- Algorithm for `split_appimage_name`:
  1. Strip a case-insensitive `.AppImage` suffix.
  2. Repeatedly strip a trailing arch token preceded by `-`, `_` or `.`: `x86_64`, `amd64`, `aarch64`, `arm64`, `i386`, `i686`, `armhf`.
  3. If the last `-`/`_`-separated segment starts with a digit, or with `v` followed by a digit, it is the version (minus the `v`) and the rest is the name. Otherwise the version is `None`.
- Behaviour:
  - Scan these dirs non-recursively: `~/Applications`, `~/.local/bin`, `~/Downloads`, `~/opt`, plus the `destination` value from `~/.config/appimagelauncher.cfg` (an INI `[AppImageLauncher]` section; `~` is expanded).
  - Only check regular files that are not symlinks. Read the first 16 bytes. Skip missing dirs and unreadable or short files silently.
  - `id = appimage:<file name>`, `location` = the in-system path, `install_size` = the file length, `installation = User`, `has_gui = true`.
  - Desktop linking: an entry links to an AppImage when its `exec_program`, after `~` expansion, equals the AppImage's in-system path, or when it `is_appimage_entry` and `normalize_name(entry name)` or `normalize_name(entry id minus ".desktop")` equals `normalize_name(appimage name)`. Linked entries fill `desktop_ids`, `display_name` and `exec_names`, and their IDs go into `claimed_desktop_ids`. An entry's own `X-AppImage-Version` fills `version` when the filename gave none.

- [ ] **Step 1: Failing tests:**
  - `split_names`, a table test:
    - `emobie_0.8.2_amd64.AppImage` gives `("emobie", Some("0.8.2"))`
    - `Grok_Bot_0.79.2.AppImage` gives `("Grok_Bot", Some("0.79.2"))`
    - `GutCheck-0.19.0-x86_64.AppImage` gives `("GutCheck", Some("0.19.0"))`
    - `Pass_Broker-x86_64.AppImage` gives `("Pass_Broker", None)`
    - `tool-v2.1.AppImage` gives `("tool", Some("2.1"))`
  - `magic_detection`: a valid header is true, a plain ELF is false, and 3 bytes is false.
  - `lists_appimages_in_applications_dir`
  - `hostile_dir_entries_skipped` (Review Focus #4): a broken symlink, a symlink to a valid AppImage, a subdir named `x.AppImage`, a zero-byte file and a non-ELF `.AppImage`. Only the one real file is listed.
  - `links_desktop_by_exec_path`
  - `links_appofholding_style_entry`: `gutcheck.desktop` with `Exec=/usr/bin/appofholding run gutcheck %U`, `Name=GutCheck` and `X-AppImage-Version=0.19.0` links to `GutCheck-0.19.0-x86_64.AppImage`, and its ID lands in `claimed_desktop_ids`.
  - `appimagelauncher_destination_scanned`
- [ ] **Step 2: Run** `cargo test -p crumbs-core --features test-support sources::appimage` and expect FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** and expect PASS.
- [ ] **Step 5: Commit** `feat(core): appimage source`

---

### Task 8: Unmanaged source and inventory discovery

**Files:** Create `crates/crumbs-core/src/sources/unmanaged.rs` and `crates/crumbs-core/src/inventory.rs`

**Interfaces:**
- Consumes: all sources, `SourceListing`, `normalize_name`.
- Produces:
  - `pub struct UnmanagedSource;` It is always available, and `is_package_db() == false`.
  - Its `list` is not called by `discover` directly. `discover` calls `pub fn list_unmanaged(cx: &SourceContext, owned: &HashSet<PathBuf>, claimed: &HashSet<String>) -> SourceListing`, and the trait `list` returns an empty listing.
  - `pub struct SourceStatus { pub source: SourceKind, pub available: bool, pub app_count: usize, pub skipped_reason: Option<String> }` (Serialize).
  - `pub struct Inventory { pub apps: Vec<App>, pub statuses: Vec<SourceStatus>, pub errors: Vec<SourceError> }` (Serialize).
  - `pub fn default_sources() -> Vec<Box<dyn AppSource>>` returns pacman, flatpak and appimage, in that order.
  - `pub fn discover(env: &Env, runner: &dyn CommandRunner, opts: &ListOptions) -> Inventory`.
  - `pub enum Resolved<'a> { Found(&'a App), Ambiguous(Vec<&'a App>), NotFound }`
  - `pub fn find_app<'a>(inv: &'a Inventory, query: &str) -> Resolved<'a>`.
- `discover` behaviour:
  - Discover desktop entries once.
  - Run every available source in order. A source that returns `Err` is recorded as a `SourceError` and doesn't stop the others.
  - Merge the `owned_paths` and `claimed_desktop_ids` sets.
  - Run unmanaged **only if at least one `is_package_db()` source was available and listed without `Err`**. Otherwise its status has `skipped_reason = Some("no supported package database found")`.
  - Sort apps by `display_name` (case-insensitive), then `id`.
- Unmanaged rule: a desktop entry becomes an app when all of these hold:
  - it `is_application`, is not `no_display`/`hidden`, and isn't `is_appimage_entry`;
  - its ID isn't claimed;
  - its `exec_program` is not `flatpak`;
  - the resolved program path exists and isn't in `owned`.
  - **Resolving the program:** an absolute path is used as-is via `env.sys`. A bare name is looked up in `TRUSTED_DIRS` and then `~/.local/bin`. A name that doesn't resolve means the entry is skipped.
  - **App fields:** `id = unmanaged:<desktop id>`, `name` = the desktop ID minus `.desktop`, `display_name` = the entry name, `installation = User` when the entry or the program lives under home and `System` otherwise, `has_gui = true`.
  - **Location:** `/opt/<first component>` when the program is under `/opt/`, otherwise the program path.
- `find_app` order, where the first step with any matches decides:
  1. exact `id`;
  2. exact `name`;
  3. exact desktop ID, with or without `.desktop`;
  4. exact `rdns_id`;
  5. case-insensitive `display_name`;
  6. `normalize_name` equality on name or display name.

  One match gives `Found`, more than one gives `Ambiguous`.

- [ ] **Step 1: Failing tests:**
  - `unmanaged_opt_app_listed`: `/opt/foo/foo` is not owned and its user entry has `Exec=/opt/foo/foo`. Expect it listed with `location == "/opt/foo"`.
  - `owned_program_not_unmanaged`
  - `claimed_and_flatpak_entries_excluded`
  - `unmanaged_skipped_without_package_db`: a fixture with no pacman db gives a status with `skipped_reason` and no unmanaged apps.
  - `failing_source_does_not_stop_others`: the flatpak runner errors, and pacman and AppImage apps are still listed alongside one error.
  - `find_app_precedence`: the query `firefox` matches the pacman package name before the Flatpak display name "Firefox".
  - `find_app_ambiguous` (Review Focus #5): two apps with display name "Firefox" and no other match give `Ambiguous` of length 2.
- [ ] **Step 2: Run** `cargo test -p crumbs-core --features test-support inventory:: sources::unmanaged` and expect FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** and expect PASS.
- [ ] **Step 5: Commit** `feat(core): unmanaged source and inventory discovery`

---

### Task 9: Escalation detection and doctor report

**Files:** Create `crates/crumbs-core/src/privilege.rs` and `crates/crumbs-core/src/doctor.rs`

**Interfaces:**
- Produces:
  - `pub enum EscalationKind { Run0, Pkexec, Sudo }` (Serialize, kebab-case).
  - `pub struct Escalation { pub kind: EscalationKind, pub program: PathBuf /* in-system */, pub requires_tty: bool }` (Serialize).
  - `pub fn detect_escalation(env: &Env, runner: &dyn CommandRunner) -> Option<Escalation>`:
    - Use `run0` if trusted-resolvable **and** the first line of `systemctl --version` parses to a systemd version of 256 or later.
    - Otherwise `pkexec`, otherwise `sudo` with `requires_tty: true`.
    - The `systemctl` call goes through the runner, and so does its trusted resolution.
  - `pub struct TrashStatus { pub path: PathBuf, pub exists: bool, pub writable: bool }`, where path = `{data_home}/Trash` and writable is checked with `rustix::fs::access(.., Access::WRITE_OK)` (`cargo add rustix --features fs`).
  - `pub struct DoctorReport { pub crumbs_version: String, pub distro: Distro, pub sources: Vec<SourceStatus>, pub escalation: Option<Escalation>, pub trash: TrashStatus, pub data_dirs: Vec<PathBuf>, pub errors: Vec<SourceError> }` (Serialize). Make `Distro` Serialize too.
  - `pub fn doctor(env: &Env, runner: &dyn CommandRunner) -> DoctorReport` reuses `discover` with default options. `crumbs_version` comes from `env!("CARGO_PKG_VERSION")`.

- [ ] **Step 1: Failing tests:**
  - `run0_requires_systemd_256`: `systemd 255 (255.4-1)` with run0 present gives pkexec when pkexec is present.
  - `run0_selected_on_261`: `systemd 261 (261.3-1-manjaro)` gives Run0.
  - `sudo_fallback_requires_tty`
  - `none_when_nothing_present`
  - `doctor_reports_sources_and_trash`
- [ ] **Step 2: Run** `cargo test -p crumbs-core --features test-support privilege:: doctor::` and expect FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** and expect PASS.
- [ ] **Step 5: Commit** `feat(core): escalation detection and doctor report`

---

### Task 10: CLI: `apps list`, `apps show`, `doctor`

**Files:** Create `crates/crumbs-cli/src/{main.rs,output.rs,cmd/apps.rs,cmd/doctor.rs}` and `crates/crumbs-cli/tests/cli.rs`. Dev-dependencies: `crumbs-core` with feature `test-support`, `assert_cmd`, `predicates`, `insta` (feature `json`), `tempfile`.

**Interfaces:**
- Consumes: `Env`, `SystemRunner`, `discover`, `find_app`, `Resolved`, `doctor`, `SourceKind`.
- CLI surface (clap derive):
  - `crumbs apps list [--source <pacman|flatpak|appimage|unmanaged>]... [--all] [--sort name|size] [--json]`
  - `crumbs apps show <app> [--json]`
  - `crumbs doctor [--json]`
  - A hidden global env var `CRUMBS_ROOT` builds `Env::fake(root)` and sets `HOME`-derived dirs from that root; otherwise `Env::from_system()`. Always use `SystemRunner`, which only finds programs inside the root because resolution is root-joined.
- `output.rs` produces:
  - `pub fn fmt_size(bytes: u64) -> String`, IEC with one decimal: `0 B`, `1023 B`, `1.0 KiB`, `1.5 MiB`.
  - `pub fn print_json<T: Serialize>(command: &str, data: &T, errors: &[SourceError])`, which writes the envelope from Global Constraints.
  - Table rendering with padded columns. Pad by `chars().count()` so non-ASCII names align. The header row is bold through anstyle.
- Human output:
  - `apps list` columns: `NAME  VERSION  SOURCE  SIZE  ID`. `--sort size` sorts descending with unknown sizes last. A footer reads `N apps`. Source errors print to stderr as `warning: <source>: <message>`.
  - `apps show` prints key/value lines: Name, ID, Source, Version, Installation, Size, Location, Desktop IDs, Executables, Description.
  - `doctor` prints the distro, one line per source (`✓ pacman  42 apps` / `✗ flatpak  not installed` / `– unmanaged  skipped: <reason>`), the escalation method or `none (privileged cleanup unavailable)`, and the trash path and status.
- Exit codes:
  - `apps list` and `doctor` exit 0, or 3 when there are source errors and at least one source succeeded.
  - `apps show` exits 2 on `NotFound` (`error: no installed app matches '<q>'`) and on `Ambiguous` (`error: '<q>' matches several apps:` followed by an `ID  NAME  SOURCE` line per candidate).
  - Clap usage errors exit 2.
  - An unexpected error prints `error: {e:#}` and exits 1.

- [ ] **Step 1: Failing tests:**
  - `output::tests::fmt_size_iec`: the four values above.
  - In `tests/cli.rs` each test builds a `FixtureTree`: pacman `firefox` with a desktop file, a pacman `coreutils`, one AppImage, an unmanaged `/opt/foo` entry, and an executable `/usr/bin/flatpak` shell script that prints the three Flatpak rows from Task 6.
  - `apps_list_snapshot`: `insta::assert_snapshot!` of stdout with the fixture path filtered out.
  - `apps_list_json_envelope`: the JSON has `schema_version == 1`, `command == "apps list"` and `data.apps` with length 6 (firefox, 3 flatpaks, 1 AppImage, 1 unmanaged).
  - `apps_list_source_filter`: `--source appimage` gives exactly one row.
  - `apps_show_found`: `apps show firefox` exits 0 and its stdout contains `pacman:firefox`.
  - `apps_show_not_found_exit_2`
  - `apps_show_ambiguous_exit_2` (Review Focus #5): add an AppImage `Firefox-1.0-x86_64.AppImage` and a Flatpak row with name `Firefox`, then query `FIREFOX`.

    The case-insensitive display-name step matches the pacman entry (display name "Firefox"), the Flatpak and the AppImage. Exact name matching is case-sensitive, so `FIREFOX` falls through to the display-name step.

    Expect exit 2, and stderr lists all three IDs.
  - `doctor_json`: `escalation` is null in the fixture and `sources[0].source == "pacman"`.
  - `flatpak_failure_exit_3`: the fake flatpak script exits 1, which gives exit 3 with a stderr warning while pacman apps still print.
- [ ] **Step 2: Run** `cargo test -p crumbs-cli` and expect FAIL.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run** `cargo insta test -p crumbs-cli --accept` once, review the snapshot by eye (correct columns, no fixture-root paths), then run `cargo test --workspace --all-features`. Expect PASS.
- [ ] **Step 5: Smoke-test on the real system.** Run `cargo run -q -p crumbs-cli -- doctor` and `cargo run -q -p crumbs-cli -- apps list`. Expect manjaro, pacman ✓, flatpak ✓, run0 detected, and the 7 AppImages from `~/Applications` listed. The command must exit within a few seconds.
- [ ] **Step 6: Commit** `feat(cli): apps list, apps show, doctor`

---

### Task 11: CI workflow and README

**Files:** Create `.github/workflows/ci.yml` and `README.md`.

- `ci.yml`: on push and PR, run on `ubuntu-latest` with stable Rust, then `cargo fmt --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings` and `cargo test --workspace --all-features`. Add a second job running the same tests in the container `archlinux:latest` (install `rust` with pacman). That job then smoke-runs `crumbs doctor --json` and `crumbs apps list --all --json` and checks with `jq -e '.schema_version == 1'`.
- `README.md`: one paragraph on what crumBS is, a status line ("M1: read-only app listing"), build instructions and the three commands.

- [ ] **Step 1: Write both files.**
- [ ] **Step 2: Validate locally** with `cargo fmt --check && cargo clippy --workspace --all-targets --all-features -- -D warnings && cargo test --workspace --all-features`. Expect everything to PASS.
- [ ] **Step 3: Commit** `ci: add workflow and README`
