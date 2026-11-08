# crumBS — Design Spec

- **Date:** 2026-11-08
- **Status:** Draft, awaiting review
- **Binary / crate name:** `crumbs` (display name: crumBS)

## 1. Purpose

crumBS is a public, cross-distro Linux tool that removes applications *completely* —
the package plus the config, cache, data, autostart and desktop-integration files it
leaves behind — in the spirit of AppCleaner on macOS. It also scans for reclaimable
space ("bloat"): caches, temp files, trash, old package caches, orphaned packages,
journal logs and similar.

It ships first as a robust CLI. A GUI comes later and is built on the same core
library; the GUI gets its own spec.

### Success criteria

1. `crumbs uninstall <app>` removes the app via the correct package manager *and*
   moves its high-confidence leftovers to the trash, after showing an exact plan.
2. `crumbs orphans` finds config/cache/data directories that belong to no installed
   app, with a confidence level per item.
3. `crumbs scan` reports reclaimable space by category in under 10 s on a typical
   desktop home directory (warm cache, SSD).
4. Nothing is deleted without a preview and a confirmation (or `--yes`). User files
   go to the freedesktop trash and can be restored with `crumbs restore`.
5. Root is only used to run trusted system tools, never crumbs' own code.
6. Works on Arch/Manjaro, Debian/Ubuntu and Fedora; ships as an AppImage, AUR
   package, `.deb`, `.rpm` and a static CLI binary.

### Non-goals (v1)

- GUI (separate spec, built on `crumbs-core`).
- Registry-style "system optimisation" (swappiness, services, startup tuning).
- Duplicate-file finding across the home directory.
- Kernel removal, bootloader changes, firmware.
- Nix, Guix, Homebrew-on-Linux, Gentoo/portage package sources.
- Scheduled / background cleaning.

## 2. Architecture

Cargo workspace (edition 2024, tracks latest stable Rust):

```
crumbs/
├── crates/
│   ├── crumbs-core/     # library: all logic, no terminal I/O
│   └── crumbs-cli/      # binary `crumbs`: clap, rendering, prompts
├── data/rules/          # curated leftover rules shipped with the app (TOML)
├── packaging/           # AppImage, PKGBUILD, deb/rpm metadata
└── docs/
```

A future `crates/crumbs-gui` depends only on `crumbs-core`.

### Core modules (`crumbs-core`)

| Module | Responsibility |
|---|---|
| `env` | `Env` struct: home, XDG dirs, `root` prefix, uid, distro info. **All** filesystem paths are resolved through it so tests can use a fake tree. |
| `model` | `App`, `AppId`, `Source`, `Finding`, `Category`, `Confidence`, `Plan`, `Action`, `Report`. All `Serialize`. |
| `sources` | One module per app source implementing `trait AppSource`. |
| `desktop` | `.desktop` file parser + XDG data-dir lookup. |
| `rules` | Loads curated and user rule files mapping apps to known paths. |
| `matcher` | Turns an `App` into leftover `Finding`s with confidence. |
| `scanners` | One module per bloat category implementing `trait Scanner`. |
| `size` | Parallel disk-usage calculation. |
| `planner` | Turns selected findings into a `Plan` of `Action`s; runs simulations. |
| `executor` | Executes a plan: trash, delete, privileged batch; writes history. |
| `privilege` | Detects escalation method and runs privileged batches. |
| `safety` | Protected-path list and pre-action path verification. |
| `history` | Append-only action log and restore. |
| `config` | Loads `~/.config/crumbs/config.toml`. |

### Library API shape

- Synchronous API; long operations take a `&Progress` callback trait object and a
  `CancelToken` so a GUI can run them on worker threads and cancel.
- All public types are `Send + Sync`.
- Errors: `thiserror` enums in core; `anyhow` only in the CLI.
- External commands are executed through a `CommandRunner` trait so tests can
  substitute canned output.

## 3. App discovery

```rust
trait AppSource {
    fn id(&self) -> SourceKind;
    fn available(&self, env: &Env) -> bool;
    fn list(&self, env: &Env) -> Result<Vec<App>>;
    fn removal_plan(&self, app: &App, env: &Env) -> Result<Vec<Action>>;
}
```

| Source | Detection | Listing method | Removal |
|---|---|---|---|
| pacman | `/var/lib/pacman/local` exists | Parse `local/*/desc` + `files` directly | `pacman -Rns <pkg>` |
| dpkg | `/var/lib/dpkg/status` exists | Parse `status`; file lists from `info/*.list` | `apt-get purge <pkg>` (`dpkg -P` if apt-get missing) |
| rpm | `rpm` in trusted path | `rpm -qa --qf` with a fixed format; `rpm -ql` per app on demand | `dnf remove <pkg>` (`zypper remove` if dnf missing; `rpm -e` last) |
| Flatpak | `flatpak` in trusted path | `flatpak list --app --columns=application,name,version,installation,size` | `flatpak uninstall [--user\|--system] --delete-data <id>` |
| Snap | `snap` in trusted path | `snap list` | `snap remove --purge <name>` |
| AppImage | always | Scan `~/Applications`, `~/.local/bin`, `~/Downloads`, `~/opt` and AppImageLauncher's configured dir for ELF files with the AppImage magic (`AI\x02` at offset 8) | Trash the file |
| Unmanaged | always | `.desktop` entries whose `Exec` target is not owned by any package and is not an AppImage (e.g. `/opt/foo` tarballs) | Path-based removal of the install dir if it is unambiguous; otherwise leftovers only |

Only GUI-facing apps are listed by default: packages that ship a `.desktop` file
with `NoDisplay!=true`, plus all Flatpak/Snap/AppImage apps. `apps list --all`
includes every package.

### App identity

Each `App` carries an identifier set used for matching:

- package name, display name, version, source, install size
- desktop file ID(s) (e.g. `org.gnome.Nautilus`)
- reverse-DNS ID (Flatpak / AppStream)
- `Exec` binary basename(s)
- `StartupWMClass`
- owned files (lazily loaded)

Desktop entries are tied to packages via file ownership from the package database.

## 4. Leftover matching

### Search locations

User (no root needed):

- `$XDG_CONFIG_HOME`, `$XDG_CACHE_HOME`, `$XDG_DATA_HOME`, `$XDG_STATE_HOME`
  (first level only, plus known second-level dirs from rules)
- `~/.<name>` dot-directories and dot-files in `$HOME`
- `~/.var/app/<flatpak-id>`
- `~/snap/<name>`
- `$XDG_CONFIG_HOME/autostart/*.desktop`
- `$XDG_DATA_HOME/applications/*.desktop` and `mimeapps.list` entries (reported,
  edited only by removing lines that reference the app's desktop IDs)
- `$XDG_DATA_HOME/icons/**` files named after desktop IDs
- `$XDG_CONFIG_HOME/systemd/user/*.service` whose `ExecStart` references the app

System (root needed, reported as privileged findings):

- `/etc/<name>` and `/opt/<name>` paths **not owned by any package**

### Confidence

| Level | Rule | Pre-selected? |
|---|---|---|
| High | Exact match of a directory/file name against package name, desktop ID, reverse-DNS ID or a curated rule | Yes |
| Medium | Case-insensitive match after normalisation (strip `-`, `_`, `.`, version suffixes) or a match on `Exec`/`StartupWMClass` only | No |
| Low | Token/substring match ≥ 4 characters | No; hidden unless `--show-low` |

A name that matches more than one installed app is downgraded one level and marked
`shared`.

### Curated rules

`data/rules/*.toml` ships mappings that name-matching cannot find, e.g.:

```toml
[[app]]
match = { package = ["firefox"], flatpak = ["org.mozilla.firefox"] }
paths = ["~/.mozilla", "~/.cache/mozilla"]

[[app]]
match = { package = ["visual-studio-code-bin", "code"] }
paths = ["~/.config/Code", "~/.vscode"]
```

Users can add rules in `~/.config/crumbs/rules.d/*.toml`; user rules take
precedence. Rule-matched paths are High confidence.

### Orphans

`crumbs orphans` lists entries in the user search locations that match **no**
installed app. Orphans are never High, since "not detected" is not "not
installed": a curated rule naming an undetected app gives Medium, and anything
modified in the last 30 days is capped at Low. Orphans are never pre-selected.

## 5. Bloat scanners

```rust
trait Scanner {
    fn category(&self) -> Category;
    fn scope(&self) -> Scope; // User | System
    fn available(&self, env: &Env) -> bool;
    fn scan(&self, env: &Env, cfg: &Config, p: &dyn Progress, c: &CancelToken)
        -> Result<Vec<Finding>>;
}
```

### User scope

| Category | What | Default |
|---|---|---|
| `cache` | Per-subdir sizes of `$XDG_CACHE_HOME`, excluding `thumbnails` and dev caches | on |
| `thumbnails` | `$XDG_CACHE_HOME/thumbnails` | on |
| `trash` | All trash dirs for the user (home + per-mount `.Trash-$uid`) — emptying is permanent and says so | on |
| `temp` | Files in `/tmp` owned by the user, older than `temp_age_days` (default 7) | on |
| `browser-cache` | Cache dirs of Firefox, Chromium, Chrome, Brave, Vivaldi, Edge (native + Flatpak) | on |
| `dev-cache` | cargo registry/git cache, npm, pnpm, yarn, pip, go build/mod cache, gradle, maven, ccache. Marked `rebuildable` | **off** (opt-in) |
| `flatpak-unused` | `flatpak uninstall --user --unused` (simulated first to list runtimes) | on |
| `appimage-old` | Older versions of the same AppImage (same name stem, lower version) | on |
| `logs` | `$XDG_STATE_HOME/**/*.log` and `~/.xsession-errors*` older than 14 days | on |

### System scope (privileged)

| Category | Distro | Command |
|---|---|---|
| `pkg-cache` | Arch | `paccache -rk<keep>` (default keep 2); `pacman -Sc` if pacman-contrib missing |
| | Debian | `apt-get clean` |
| | Fedora | `dnf clean packages` |
| `orphan-pkgs` | Arch | `pacman -Rns <explicit list from pacman -Qdtq>` |
| | Debian | `apt-get autoremove --purge` |
| | Fedora | `dnf autoremove` |
| `journal` | systemd | `journalctl --vacuum-size=<journal_max>` (default 500M) |
| `coredumps` | systemd | remove files in `/var/lib/systemd/coredump` |
| `system-temp` | systemd | `systemd-tmpfiles --clean` (handles `/tmp` and `/var/tmp` safely) |
| `flatpak-unused-system` | any | `flatpak uninstall --system --unused` |

Each system finding reports its estimated reclaimable size (computed without root
where readable, otherwise from the tool's own simulation output, otherwise
"unknown").

### Size calculation

- Allocated size (`st_blocks * 512`), not apparent size.
- Parallel traversal (`rayon`); does not cross filesystems; does not follow symlinks.
- Hard links counted once per `(dev, ino)`.
- Unreadable entries are counted as errors in the report, not fatal.

## 6. Plan → preview → execute

```
scan / match  →  Vec<Finding>  →  select  →  Plan  →  simulate  →  preview  →  confirm  →  execute  →  Report
```

### Actions

| Action | Used for | Root? |
|---|---|---|
| `Trash(path)` | All user-owned files (default) | no |
| `Delete(path)` | User files with `--permanent`; emptying trash; files the trash cannot hold | no |
| `EditFile(path, remove_lines)` | Removing an app's entries from `mimeapps.list` (backup written first) | no |
| `UserCommand(argv)` | `flatpak --user` operations | no |
| `PackageRemove(source, names)` | Package-manager uninstall | yes (Flatpak `--user`: no) |
| `PrivilegedCommand(argv)` | System scanners, deleting unowned `/etc` / `/opt` paths | yes |

### Simulation

Before preview, the planner asks the package manager what will happen and puts
the real result into the plan, so the user confirms what will actually be removed:

- pacman: `pacman -Rns --print --print-format '%n %v' <pkgs>`
- apt: `apt-get -s purge <pkgs>`
- dnf: `dnf remove --assumeno <pkgs>` (parse the transaction summary)
- flatpak unused: `flatpak uninstall --unused --noninteractive --dry-run` if
  supported, else list from `flatpak list --runtime` vs app dependencies

If a simulation pulls in packages the user did not select (dependents, recursive
deps), the preview lists them explicitly under "Also removed".

### Execution order

1. Check whether the app is running (`/proc/*/exe`, `/proc/*/cmdline`); warn and
   require a second confirmation (or `--force`).
2. Privileged batch (one escalation prompt).
3. User actions (trash / delete / edit).
4. Write history entries; print report.

A failing action does not stop independent actions; the report lists every
failure and the exit code is `3` (partial failure).

## 7. Privilege model

- crumbs never runs its own code as root. (A FUSE-mounted AppImage is usually not
  readable by root anyway.)
- Privileged actions are argv vectors for **trusted system tools**, resolved to
  absolute paths only from `/usr/bin`, `/usr/sbin`, `/bin`, `/sbin` — never from
  `$PATH`, so a user-writable `~/.local/bin/paccache` cannot be run as root.
- All privileged actions in a plan are joined into one `/bin/sh` script, shown
  in full in the preview, and run with a single escalation. Every argument is
  shell-escaped. Commands run independently (no `set -e`): after each one the
  script prints a marker line `@@crumbs <index> <exit-status>` so the executor
  can report per-action success or failure. Escalation order:
  1. `run0 --` (systemd ≥ 256, usable in this session)
  2. `pkexec`
  3. `sudo` (only when stdin is a TTY)
  If none works, crumbs prints the exact commands for the user to run manually
  and exits `5`.
- Package-manager commands are passed `--noconfirm` / `-y` only after the user
  has confirmed the simulated result.
- Deleting unowned `/etc` / `/opt` paths uses `rm -rf --one-file-system --`
  only after verifying, without root, that the path and every parent are
  root-owned and not group/world-writable (so an unprivileged user cannot swap
  them via symlink between check and use).

## 8. Safety

- **Dry-run first:** destructive commands always show the plan. `--dry-run`
  stops after the preview. `--yes` skips the confirmation prompt but never the
  safety checks.
- **Protected paths** (never acted on, even if matched or listed in a rule).
  Two kinds:
  - *Exact* — the path itself is protected, children may be acted on:
    `$HOME`, every XDG base dir, `/`, `/etc`, `/opt`, `/var`, `/home`.
  - *Subtree* — the path and everything below it is protected:
    `~/.ssh`, `~/.gnupg`, `~/.pki`, `~/.local/share/keyrings`,
    `~/.password-store`, `~/.config/crumbs`, `~/.local/state/crumbs`,
    `~/Documents`, `~/Desktop`, `~/Pictures`, `~/Music`, `~/Videos`, `/usr`,
    `/boot`, `/var/lib/pacman`, `/var/lib/dpkg`, `/var/lib/rpm`.
  Users can add more subtree paths in `config.toml` (`protect = [...]`); they
  cannot remove built-ins.
- **Pre-action verification:** immediately before each action, re-`lstat` the
  path and compare `(dev, ino)` with the scan result; refuse on mismatch, on
  symlink-to-elsewhere, or when the path falls outside the allowed roots for its
  finding.
- **Trash-first:** user files go to the freedesktop trash
  (`$XDG_DATA_HOME/Trash` or the mount's `.Trash-$uid`). If the item cannot be
  trashed (e.g. a mount with no usable trash dir), it is skipped with an error
  unless `--permanent` is set.
- **History:** every action is appended to
  `~/.local/state/crumbs/history.jsonl` with timestamp, action, original path,
  trash location, size and result.
- **Restore:** `crumbs restore <history-id|path>` moves trashed items back.
  Package removals are not reversible; the preview says so.

## 9. CLI

```
crumbs apps list [--source <s>] [--all] [--sort size|name] [--json]
crumbs apps show <app> [--show-low] [--json]           # details + leftovers
crumbs uninstall <app>... [--leftovers-only] [--keep-data] [--show-low]
                 [-i] [--dry-run] [--yes] [--permanent] [--force]
crumbs orphans [--show-low] [--clean] [-i] [--dry-run] [--yes] [--json]
crumbs scan [--category <c>]... [--user|--system] [--json]
crumbs clean [<category>...] [--user|--system] [-i] [--dry-run] [--yes] [--permanent]
crumbs history [--limit N] [--json]
crumbs restore <id|path>... [--dry-run]
crumbs doctor [--json]       # sources detected, escalation method, trash status, distro
```

- `<app>` is resolved by exact package name, desktop ID, Flatpak ID or display
  name; ambiguous names list the candidates and exit `2`.
- `-i` opens an interactive checklist (pre-selected = High confidence).
- `--keep-data` keeps config/data, removes only cache.
- Output: human tables by default, coloured when stdout is a TTY
  (`NO_COLOR` respected); sizes in IEC units.
- `--json` writes one JSON document to stdout:
  `{"schema_version": 1, "command": "...", "data": {...}, "errors": [...]}`.
  The schema is documented in `docs/json-schema.md` and versioned.

### Exit codes

| Code | Meaning |
|---|---|
| 0 | Success |
| 1 | Error |
| 2 | Usage error / ambiguous app |
| 3 | Partial failure |
| 4 | Aborted by user |
| 5 | Privilege escalation unavailable or denied |

## 10. Configuration

`~/.config/crumbs/config.toml` (optional; all keys have defaults):

```toml
temp_age_days = 7
log_age_days = 14
pkg_cache_keep = 2
journal_max = "500M"
categories_off = ["dev-cache"]
protect = []          # extra protected paths
exclude = []          # paths/globs never reported
appimage_dirs = []    # extra AppImage locations
```

## 11. Testing

- **Fake system trees:** `Env` points at a `tempfile` root containing a fake
  `$HOME`, `/var/lib/pacman/local`, `/var/lib/dpkg/status`, `.desktop` files,
  AppImage stubs, etc. Fixtures live in `crates/crumbs-core/tests/fixtures/`.
- **Command runner fake:** canned output for `rpm`, `flatpak`, `snap`, `pacman
  --print`, `apt-get -s`, `dnf --assumeno`, escalation tools.
- **Unit tests:** parsers, matcher confidence rules, protected-path checks,
  pre-action verification (including a symlink-swap test), shell escaping of
  privileged scripts.
- **CLI tests:** `assert_cmd` + `insta` snapshots of human and JSON output
  against fixture trees.
- **Cross-distro CI (GitHub Actions):** run the test suite plus read-only
  smoke tests (`crumbs doctor`, `crumbs apps list --json`, `crumbs scan --json`)
  inside `archlinux`, `debian:stable`, `ubuntu:latest` and `fedora:latest`
  containers.
- **No test ever deletes outside its temp root.** `Env` refuses a non-temp
  `root` when built with `cfg(test)`.

## 12. Packaging and release

- **Static CLI:** `x86_64-unknown-linux-musl` release build, stripped.
- **AppImage:** built with `appimagetool` using the static type-2 runtime (no
  `libfuse2` needed on the host). `AppRun` execs the static binary; includes a
  `.desktop` file and icon. Terminal app (`Terminal=true`) until the GUI exists.
- **AUR:** `packaging/aur/PKGBUILD` (`crumbs` source build, `crumbs-bin`).
- **.deb / .rpm:** `cargo-deb` and `cargo-generate-rpm` metadata in
  `crumbs-cli/Cargo.toml`.
- **Release workflow:** tag `v*` → build all artifacts → GitHub release with
  SHA256 sums.
- aarch64 builds are added after x86_64 is stable.

## 13. Milestones

Each milestone gets its own implementation plan and ends in a working, tested CLI.

1. **Foundation** — workspace, `Env`, model, `CommandRunner`, desktop parser,
   pacman + Flatpak + AppImage + unmanaged sources, `apps list`, `apps show`
   (without leftovers), `doctor`. — done
2. **Leftovers & uninstall (user level)** — rules loader + curated rules,
   matcher, safety module, trash + history, `apps show` leftovers,
   `uninstall --leftovers-only`, AppImage uninstall, `orphans`, `history`,
   `restore`. — done
3. **Bloat (user level)** — size module, user scanners, `scan`, `clean --user`. — done
4. **Privilege & system** — privilege module, simulations, pacman and
   Flatpak uninstall through one escalation prompt, system scanners
   (`pkg-cache`, `orphan-pkgs`, `journal`, `coredumps`, `system-temp`,
   `flatpak-unused-system`), `clean --system`. apt and dnf variants of the
   package categories move to milestone 5 with their sources. — done
   - **4b. System leftovers and unmanaged removal** — `/etc/<name>` and
     `/opt/<name>` leftovers not owned by any package, path-based removal of
     unmanaged apps, and the owned-directory database both need. They are the
     riskiest deletions and get their own plan; scope in
     `docs/superpowers/notes/m4b-scope.md`.
5. **Cross-distro** — dpkg, rpm, Snap sources; distro CI matrix.
6. **Packaging** — musl build, AppImage, AUR, deb, rpm, release workflow.

The GUI follows as a separate spec once milestones 4 and 4b are complete.
