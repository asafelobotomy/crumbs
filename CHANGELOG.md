# Changelog

## Unreleased

- **uQ Settings, uninstalling in a dialog:** Uninstall… opens a dialog with the app, the choice of how much goes and everything that would be removed. Once you press Uninstall it shows a progress bar while crumBS works, then lists what was removed (grouped as the app, your files, settings, caches and menu entries) and anything that stayed, with why, and closes with OK. An app that is open is still never forced: the dialog says so and offers "Close it and uninstall".
- **uQ Settings, uninstalling several apps:** every app that can be removed has a tick box. "Uninstall selected…" opens the same dialog for all of them: one choice of how much goes, each app's plan behind "What goes", then they are uninstalled one after another with a progress bar for the queue ("Uninstalling Notes (2 of 3)…"), and a summary of what went for each. uQ still asks once for each app; a no skips that app and the queue goes on, and "Stop after this one" skips the rest.
- **uQ Settings, Overview:** "Space you can free" (and "System space", once checked) opens with a ring chart of the space by kind, the biggest six and "Everything else", with a legend giving each one's size and share.

## 0.8.3

- The desktop entry lists one main menu category (`System;Filesystem;`), so menus no longer show crumbs twice.

## 0.8.2

- **No more endless "Working…":** without a terminal (uQ's Storage page, `crumbs mcp`), a privileged run now gives up when the password prompt gets no answer within 2 minutes: the escalation program is stopped, polkit drops its dialog, and crumbs reports exit 5 ("no answer to the password prompt") instead of waiting for ever. Once the root shell runs, there is still no limit. While an uninstall or system cleanup runs, the page also says to enter your password in the dialog.

## 0.8.1

- **System leftovers no longer include software in use:** `orphans --system` (and uQ's "System leftovers") listed every unowned folder in `/opt`, so a hand-installed SDK or runtime linked from `/usr/local/bin` (`gcloud -> /opt/google-cloud-sdk/...`), named by a link in `/opt` itself (`/opt/gradle -> gradle-8.14.3`), on your `PATH` (`/opt/node22/bin`) or running right now was offered for deletion as root. Those are now left out.
- **Snap data:** `~/snap/<name>` is matched to the installed snap of that name. Before, it was never matched: an installed snap's data was listed as an orphan ("does not match any installed app", with `--show-low`) and `uninstall snap:<name>` left it behind.
- **uQ Settings:** a "Put back" that finds nothing to restore now says why ("Couldn't put this back: no matching history entry") instead of "Something went wrong". The page's tests follow uQ v0.3.21's test kit, and CI checks the page with that release's `uqctl`.

## 0.8.0

- **uQ Settings, Apps tab redesigned:** apps are sorted into menu categories (Internet, Multimedia, Graphics, Office, Games, Development, Utilities, System, Other), each folding away with its app count and size; the biggest opens first and a search opens them all. Each app is a framed tile with its icon, name, source, size and its own Uninstall… button inside the frame (no more button at the far edge of a wide row, next to the wrong app), two tiles to a line on a wide window. An open preview takes the whole line. Needs crumbs with `category` (below); with an older crumbs the apps are sorted by source instead.
- **uQ Settings, the other tabs:** Overview, Leftovers and History rows are framed tiles too, with the size and the button together on one side of the tile, so a button is always next to what it acts on.
- **`category` on every app (JSON):** `apps list` and `apps show` carry `category`, the menu group of the first launcher whose `Categories=` names one (null when none does).

## 0.7.0

- **Uninstalling an app that is open:** `uninstall --close-running` asks a running app to quit (SIGTERM, never SIGKILL), waits up to 10 seconds and refuses as before if it stays up. uQ's Storage and cleanup page keeps the uninstall preview open when crumBS refuses, says plainly that the app is open (a tray app such as CopyQ always is) and offers "Close it and uninstall", instead of closing the preview and leaving the person with nothing to do.

- **Sizes on compressed btrfs:** btrfs reports compressed files at their uncompressed size, so a Rust `target` folder could be listed at three times the space it takes. `scan` now marks such sizes `≤` with a note, and `clean`, `uninstall` and `orphans --clean` previews say the space freed can be smaller. JSON: `size_uncompressed` on a BloatItem, `uncompressed_sizes` on a CategoryReport and `uncompressed` in a Finding's `size`, each present only when set. Running as root, crumbs reads the real on-disk size, as `compsize` does.
- **`build-output`:** also finds electron-builder's `dist` (beside its config file) and Godot's `.godot` import cache (beside `project.godot`).
- **Orphans:** no longer lists data of installed software under another name: folders named after an installed program (`~/.config/gh`, `~/.cargo`, `~/.npm`), AUR variants' files under the plain name (`paru-bin`), the apps in appOFholding's library (GutCheck's settings, say), the desktop's shared files (`kdeglobals`, `recently-used.xbel`, `user-places.xbel`, `pki`) and the `containers` storage. Go's build cache and settings and the NVIDIA shader cache are now their packages' leftovers.
- **uQ:** `~/.local/share/uq` and uq-dev's `~/.local/share/uq-dev` (uq), and `~/.local/share/notifd.prev`, `~/.local/state/notifd` and `~/.local/state/notify` (noti-fi) are claimed, so they are never offered as orphans.

## 0.6.0

- **`crumbs config`:** `show`, `set`, `add`, `remove` and `reset` read and change `~/.config/crumbs/config.toml` one key at a time, refusing a value the config would not accept before anything is written. With `--json` the result lists every value in effect and every key with the kind of value it takes, so uQ's Storage and cleanup page can offer the settings.
- **App icons:** apps in `apps list` and `apps show` (JSON) carry `icon`, the `Icon=` of their launcher (a Flatpak app's id when it has none).
- **uQ Settings page, rebuilt:** Storage and cleanup now has the tabs and cards of Settings' own pages (Overview, Apps, Leftovers, History, Preferences) and needs a uQ with `uq_api = 2`. Every long read shows a progress bar; the space you can free shows a share bar per category; apps show their icons. Uninstall… first shows what would be removed, grouped and saying what goes to the trash and what goes for good, and lets you keep your settings and data or remove only the leftovers. New: system leftovers in `/opt` (deleted with your password), and a Preferences tab for crumBS's own settings through `crumbs config`.
- **Uninstall groups:** each action of an `uninstall` or `orphans --clean` plan carries `group` in JSON (`app`, `data`, `settings`, `cache` or `launchers`), so a preview can say plainly what goes.

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
