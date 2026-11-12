# Changelog

## Unreleased

## 0.5.0

- **uQ Settings page:** Storage and cleanup (`uq/pages/crumbs/`), under Apps in uQ Settings: space you can free (user files, and the system on request) with a Clean up button per category, leftovers of removed apps, recent cleanups with Put back, and your apps by size with Uninstall…. It runs the existing `crumbs … --json` commands, asks before every change, never forces one, and shows crumBS's own refusals as they are. The `crum-bs` and `crum-bs-bin` packages install it (the `.deb` and `.rpm` do not: uQ is Arch-only); `crum-bs-bin` now also installs the "Uninstall…" action, and builds from the release's source tarball for that.
- **`uninstall --desktop-id <id>`:** removes the app that owns a launcher, named by its desktop file id (`firefox.desktop`) and nothing else. An id that is not a plain desktop file id, or that no app or several apps own, is refused with exit 2. The usual preview, `Proceed?` and the second question for uQ's own packages still apply.
- **uQ:** an "Uninstall…" app action (`uq/launcher/actions/crumbs-uninstall.appaction`) on every app row in uQ's launcher, running `crumbs uninstall --desktop-id` in a terminal. The `crum-bs` package and the `.deb` install it to `/usr/share/uq/launcher/actions/`.

## 0.4.0

- **`build-output`** (off by default): build folders git ignores in your projects: Cargo's `target`, `node_modules` and web bundler caches, Gradle's and CMake's `build`, a project's `.gradle`, Python `.venv`s. Recent ones and any in a repository a process works in are not preselected. New config keys `project_dirs` and `build_age_days`.
- **`containers`:** podman's and Docker's untagged images (pruned by the engine) and unused volumes (never preselected).
- **`aur-build`:** pamac's build trees in `/var/tmp` (or its `BuildDirectory`), deleted permanently while no process works in them.
- **`app-cache`:** the Chromium caches of Electron apps in `~/.config` (Discord, VS Code, Cursor, Claude, ...), held back while the app runs.
- **`pkg-cache`:** also every cached version of packages no longer installed (`paccache -ruk0`).
- **Deleting** a tree that holds read-only folders of yours (Go's module cache) now completes: their write permission is restored first, by file descriptor, never outside the tree.
- **`snapshots`** (system, opt-in): Timeshift snapshots beyond the newest `snapshots_keep` (3), deleted by Timeshift in the root step.

## 0.3.0

- **uQ:** never offers uQ's, scdr's, noti-fi's or appOFholding's own files as leftovers or orphans. A package that is part of the uQ desktop (uq, uq-hyprland-pin, scdr, noti-fi, crum-bs) is removed only when asked twice: a second question at a terminal, `--desktop` otherwise (`desktop: true` over MCP). `apps show` and the JSON `part_of` field say which packages those are.
- **appOFholding:** once it is in use, the AppImages in its library (`~/Applications`) are its own: crumbs no longer lists them, offers their old versions to `appimage-old` or uninstalls them.
- **Releases:** `SHA256SUMS` is signed with minisign (`SHA256SUMS.minisig`, key in `minisign.pub`), and the release build pins appimagetool and the AppImage runtime by version and checksum.
- **Cache safety:** a folder in `~/.cache` that holds a git working tree, or that a running process works in, is no longer preselected (`contains a git checkout (...)`, `in use by cargo (pid ...)`); a git checkout is not counted as rebuildable.
- **`clean --exclude <pattern>`:** leaves out what a pattern matches for one run, as the config's `exclude` does; `preview_clean` takes `exclude` over MCP.
- **Full disks:** over 90% full, a clean that would move things to the trash says it frees nothing until the trash is emptied and points at `--permanent`.
- **Measured cleanup:** what `paccache`, `pacman -Sc`, `apt-get clean`, `dnf clean`, `journalctl --vacuum-*`, `systemd-tmpfiles --clean` and `flatpak uninstall --unused` free is measured and reported (`freed` on the result in JSON).
- **`flatpak-unused`:** lists the runtimes Flatpak would remove, by name, and nothing when there are none, instead of an item of unknown size every time.

## 0.2.0

The first public release.

- **Apps:** lists apps from pacman, dpkg, rpm, Flatpak, Snap, AppImages and unmanaged installs, with their sizes and leftovers (`crumbs apps list`, `crumbs apps show`).
- **Uninstall:** removes an app with its leftovers, after a preview, through the system package manager. Root steps run as one batch with a single prompt (run0, pkexec or sudo).
- **Clean:** finds and removes caches, logs, trash, temp files and system bloat such as package caches and orphan packages (`crumbs scan`, `crumbs clean`, `crumbs orphans`).
- **Safety:** every deletion is checked against safety scopes, moved to the trash where possible, recorded in the history and can be restored (`crumbs restore`).
- **MCP:** `crumbs mcp` serves the read and preview tools over stdio; `apply` asks you to confirm each plan.
- **Distros:** Arch and Manjaro, Debian and Ubuntu, and Fedora.
- **Packages:** an AppImage that needs no libfuse2, `.deb` and `.rpm` packages, the AUR packages `crum-bs` and `crum-bs-bin`, and a static x86_64 binary.
