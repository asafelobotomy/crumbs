# crumBS

crumBS is a Linux command-line tool that lists the apps installed on your system across pacman, Flatpak, AppImage and unmanaged `.desktop` sources, finds the files they leave behind, finds space you can reclaim (caches, the trash, old temporary files, logs, superseded AppImages), and moves the items you pick to the trash. It is pre-alpha software.

**Status:** M4: `uninstall` fully removes pacman and Flatpak apps (and AppImages), and `scan` and `clean` handle system bloat with `--system`, on top of M3's reclaimable space in your own files and M2's leftovers, orphans, history and restore. Everything that needs administrator rights runs in one batch behind one prompt. Not available yet: removing unmanaged (path-based) apps, and system-wide leftovers (`/etc`, `/opt`); both are planned as M4b. apt, dnf and Snap come later.

## Build

```sh
cargo build --release
./target/release/crumbs --help
```

## Commands

- `crumbs apps list [--source <source>] [--all] [--sort name|size] [--json]` lists installed apps.
- `crumbs apps show <app> [--show-low] [--json]` shows one app and its leftovers.
- `crumbs uninstall <app>... [--leftovers-only] [--keep-data] [--show-low] [-i] [--dry-run] [--yes] [--permanent] [--force] [--json]` removes an app and its leftovers. pacman apps are removed with `pacman -Rn` (the exact package list its simulation printed, dependencies included; the preview shows `also removed: ...`), Flatpak apps with `flatpak uninstall` for their installation (user, system or a named one), AppImages by trashing the file. Unmanaged apps need `--leftovers-only`. With `--leftovers-only` the app stays, and so do its launchers, icons and file-type associations. `--keep-data` keeps the app's settings and data and removes only its caches; when the AppImage file itself goes, its own launchers and icons go with it, so nothing is left pointing at a removed file.
- `crumbs orphans [--show-low] [--json]` lists per-user files that belong to no installed app.
- `crumbs orphans --clean (--select <path>... | -i) [--dry-run] [--yes] ...` removes chosen orphans. Nothing is removed unless you name it.
- `crumbs history [--limit N] [--json]` lists past removals, newest first, each batch labelled with the app it uninstalled, `orphans`, `clean` or `restore`.
- `crumbs restore <batch | entry | path>... [--dry-run] [--json]` puts items back. Restoring a batch skips what was permanently deleted, run as a command or removed as a package; naming such an entry by its ID says why it cannot come back.
- `crumbs scan [--category <name>]... [--details] [--user | --system] [--json]` lists reclaimable space by category. `--system` lists the system categories instead. It only reads: it changes nothing, needs no administrator rights and writes no history. With `--details` every item is listed. Naming categories scans exactly those, even ones switched off in the config.
- `crumbs clean [<category>...] [-i] [--dry-run] [--yes] [--permanent] [--force] [--user | --system] [--json]` frees the space `scan` found. With no category it cleans every enabled user category; `--system` cleans the system categories instead. Naming categories cleans exactly those, and naming a system category implies `--system`. `-i` opens a checklist over all items. `--user` with `--system`, or a category of the other kind, is a usage error (exit 2).
- `crumbs doctor [--json]` reports which sources and tools crumbs can use.

### Categories

`scan` and `clean` use these category names, always listed in this order:

| Name | What it is | On by default |
|---|---|---|
| `cache` | one item per app in `~/.cache` | yes |
| `thumbnails` | `~/.cache/thumbnails` | yes |
| `trash` | the entries in your home trash (`~/.local/share/Trash`) | yes, but only emptied when named |
| `temp` | your own old files in `/tmp` | yes |
| `browser-cache` | cache folders of Firefox, Chromium, Chrome, Brave and others, native and Flatpak | yes |
| `dev-cache` | build-tool caches (cargo, npm, pip, go, gradle, ...) that the tool recreates | no |
| `flatpak-unused` | Flatpak runtimes no installed app uses | yes, but only runs when named |
| `appimage-old` | superseded AppImage versions (the newest, any version a launcher points at and any running one are kept) | yes |
| `logs` | old `.log` files under `~/.local/state` | yes |

An item whose app is running (a cache, a browser cache, an old AppImage) is listed but not selected, and says so. Emptying the trash cannot be undone, so a plain `crumbs clean` never plans it: name the category (`crumbs clean trash`) or tick it with `-i`.

### System categories

`crumbs scan --system` lists these, and `crumbs clean --system` (or naming one) cleans them. Scanning needs no administrator rights; cleaning does.

| Name | What it is | Runs by default with `clean --system` |
|---|---|---|
| `pkg-cache` | old package versions in pacman's cache (`paccache -rk<pkg_cache_keep>`; without `paccache`, `pacman -Sc`, which also drops unused sync databases) | yes |
| `orphan-pkgs` | packages installed as dependencies that nothing needs (`pacman -Qdtq`), removed as the exact closure `pacman -Rs --print` showed | no: only when named |
| `journal` | the systemd journal, vacuumed to `journal_max` when it is larger | yes |
| `coredumps` | `core.*` files in `/var/lib/systemd/coredump` | yes |
| `system-temp` | `systemd-tmpfiles --clean --prefix=/tmp --prefix=/var/tmp`: stale files by the tmpfiles.d age rules, size unknown | yes |
| `flatpak-unused-system` | Flatpak runtimes no app uses, in the system installation (size decided by Flatpak) | no: only when named |

The two opt-in categories are never run by a plain `crumbs clean --system`; name them (`crumbs clean orphan-pkgs`) or tick them with `-i`. A category whose tool is missing (no pacman, no journalctl, no coredump directory) is shown as `unavailable (<reason>)`, which is not an error.

System cleanup is never moved to the trash: the commands run as root and cannot be undone, and the preview says so. Example (a throwaway test tree):

```
$ crumbs scan --system
CATEGORY               ITEMS  SIZE       NOTE
pkg-cache              1      86.2 MiB   rebuildable
orphan-pkgs            1      7.6 MiB
journal                1      219.1 MiB
coredumps              1      12.0 KiB
system-temp            1      ?
flatpak-unused-system  1      ?

reclaimable: 312.9 MiB+ across 6 items

$ crumbs clean --system --dry-run
Clean
ACTION  SIZE      PATH                                                      REASON
run     —         paccache -rk2                                             package cache (keep 2 versions)
    - as root: '/usr/bin/paccache' '-rk2'
run     —         journalctl --vacuum-size=500M                             vacuums to 500M
    - as root: '/usr/bin/journalctl' '--vacuum-size=500M'
delete  12.0 KiB  /var/lib/systemd/coredump/core.foo.1000.abc.1.zst         core.foo.1000.abc.1.zst
    - as root: '/usr/bin/rm' '-f' '--' '/var/lib/systemd/coredump/core.foo.1000.abc.1.zst'
run     —         systemd-tmpfiles --clean --prefix=/tmp --prefix=/var/tmp  removes files past the tmpfiles.d age limits under /tmp and /var/tmp
    - as root: '/usr/bin/systemd-tmpfiles' '--clean' '--prefix=/tmp' '--prefix=/var/tmp'
note: these steps run with administrator rights; you will be asked once

will permanently delete 1 item (12.0 KiB)
will run 3 commands
```

### Administrator rights

Only these need them:

- removing a pacman app (`pacman -Rn`), a system Flatpak app or a Flatpak app in a named installation;
- the system categories (`clean --system`);
- deleting a system file (a core dump).

Removing a per-user Flatpak app, an AppImage, leftovers and every user category need none. A run that has such steps asks **once**: crumbs joins them into one script and runs it through `run0` (on systemd 256 or newer), else `pkexec`, else `sudo` (`sudo` only when it can ask on a terminal). The preview says `these steps run with administrator rights; you will be asked once`, and shows each command exactly as it will run. `--dry-run`, `scan` and `apps` never ask.

Exit 5 means administrator rights were needed and nothing that needs them was changed. It has two causes, and crumbs prints the commands to run yourself in both:

- No escalation program is available. crumbs stops after the preview, before any prompt, writes no history, and prints `error: no way to get administrator rights` followed by the commands.
- The prompt was cancelled or denied. The attempt is recorded in the history and leftovers that depended on the removal are skipped.

The second case looks like this:

```
$ crumbs uninstall firefox --yes
...
error: administrator rights were not granted; nothing that needs them was changed
to do it yourself, run:
  sudo '/usr/bin/pacman' '-Rn' '--noconfirm' 'firefox' 'gn' 'libjuice'
```

With `--json` they are in `data.manual_commands`. Leftovers that depend on a removal that did not happen are left alone too, and never listed.

Example of the preview for a pacman app (a throwaway test tree):

```
$ crumbs uninstall firefox --dry-run
Firefox  pacman:firefox
ACTION  SIZE     PATH                       REASON
remove  —        pacman: firefox 159.0.1-1  the app itself
    - as root: '/usr/bin/pacman' '-Rn' '--noconfirm' 'firefox' 'gn' 'libjuice'
trash   4.0 KiB  ~/.cache/firefox           named after the app name
also removed: gn 0.2385.1-1, libjuice 1.7.3-1
note: this cannot be undone by crumbs
note: these steps run with administrator rights; you will be asked once

will move 1 item (4.0 KiB) to the trash
```

A removed package is recorded in `history`, but crumbs cannot restore packages; reinstall them with your package manager. The leftovers go to the trash as usual and `crumbs restore` brings them back.

### Trash and freed space

`clean` moves items to the freedesktop trash, the same as `uninstall`. A file in the trash still takes disk space until the trash is emptied, so `clean` says `moved 56.0 KiB to the trash`, not `freed`; `crumbs clean trash` empties the trash, and `--permanent` deletes everything straight away. The two exceptions are always permanent deletes, and the preview says so: `temp` (files in `/tmp`) and `trash` (emptying the trash). `flatpak-unused` runs `flatpak uninstall --user --unused`; Flatpak decides what goes, so crumbs can only report `freed space decided by flatpak`. That cannot be undone, and neither can a permanent delete; the closing `undo with: crumbs restore ...` line appears only when something restorable was trashed.

Example (a throwaway test tree):

```
$ crumbs clean cache trash temp --dry-run
Clean
ACTION  SIZE     PATH                                         REASON
trash   4.0 KiB  ~/.cache/foo                                 foo
delete  8.0 KiB  ~/.local/share/Trash/files/old.txt           ~/docs/old.txt
delete  0 B      ~/.local/share/Trash/info/old.txt.trashinfo  ~/docs/old.txt
delete  8.0 KiB  /tmp/build-output.bin                        deleted permanently (temporary file)
note: space is freed when the trash is emptied (or use --permanent)
note: permanently delete (temporary files)
note: permanently delete (emptying the trash)

will move 1 item (4.0 KiB) to the trash
will permanently delete 3 items (16.0 KiB)
```

### Configuration

Optional keys in `~/.config/crumbs/config.toml`:

- `categories_off = ["dev-cache"]` lists the categories `scan` and `clean` skip unless you name them. The default is `["dev-cache"]`; an unknown name is ignored with a warning.
- `temp_age_days = 7`: how many days a file in `/tmp` must have gone unmodified and unchanged to be listed under `temp`. The later of its modification and change times counts (and for a folder, of everything in it), so files just unpacked from an archive with old dates are not old.
- `log_age_days = 14`: the same for `logs`.

Both must be at least 1; a smaller value is replaced by 1 with a warning (`config.toml: temp_age_days must be at least 1; using 1`).

- `pkg_cache_keep = 2`: how many versions of each package `pkg-cache` keeps. 0 is allowed; a negative value is replaced by 2 with a warning.
- `journal_max = "500M"`: the size `journal` vacuums the systemd journal down to; `journal` lists nothing while the journal is at or below it. Digits with an optional `K`, `M`, `G` or `T` (`^[0-9]+[KMGT]?$`), above 0; anything else is replaced by `500M` with a warning.

A `..` in a `protect`, `exclude` or `appimage_dirs` entry is resolved as the system would: `/../x` is `/x`.

Exit codes: 0 ok, 1 error, 2 usage error or ambiguous app, 3 partial failure, 4 aborted by the user, 5 administrator rights needed but unavailable or denied.

## Safety

- **Trash first.** Files go to the freedesktop trash, not the void. Only `--permanent` deletes, and it says so in the preview. The exceptions are emptying the trash and clearing `/tmp` (`clean trash`, `clean temp`), which are always permanent and say so, as are the system categories and package removals (they run commands as root).
- **Careful with `/tmp`.** `temp` only runs when `/tmp` is owned by root and has the sticky bit. Only your own files and folders directly in `/tmp` are considered, only when older than `temp_age_days`, and never sockets, pipes, devices, symlinks, names that look like live sessions (X, tmux, ssh, dbus, ...) or anything a running process has open. Each is checked again right before it is deleted, and `temp` refuses to run as root.
- **Privileged steps are simulated, verified and quoted.** A pacman removal is first simulated as you with `pacman -Rs --print`; if pacman refuses (a dependency would break), crumbs refuses too. The real command then names exactly the packages the simulation showed (`-Rn --noconfirm <names>`, never `-s`), so it cannot remove more than the preview listed. A pacman database lock (`db.lck`) stops the run (`pacman is busy`) before anything is asked. Afterwards crumbs reads the pacman database again, and a package still installed is reported as a failure, whatever the command said. Every program runs by its absolute path from the system directories with a fixed `PATH` and `LD_PRELOAD`/`LD_LIBRARY_PATH` cleared, every argument is single-quoted, names are checked to be plain package or app names, and a system file is checked again right before it is deleted as root. Flatpak is always told which installation (`--user`, `--system` or `--installation=<name>`) and never `--delete-data`: an app's data in `~/.var/app/<id>` is trashed as a restorable leftover instead. In tests and fake roots crumbs never escalates at all.
- **The history is private.** `history.jsonl` is opened without following symlinks; a symlinked directory or file is refused, and a looser mode is made owner-only (group and other bits cleared) with a warning.
- **Preview before anything happens.** `uninstall`, `orphans --clean` and `clean` show every path with its size and the reason it was chosen, then ask. `--dry-run` shows the plan and stops.
- **No surprises in scripts.** Without a terminal, `--yes` is required (or `--dry-run`); `--json` needs one of the two as well.
- **Protected paths.** crumbs refuses system locations, your home directory itself and other paths that should never be removed, re-checks every path right before acting, and skips anything that changed since the scan. Two keys in `~/.config/crumbs/config.toml` add your own (paths start with `/` or `~/`):
  - `protect = ["~/.config/keepme"]`: that path, everything under it and every folder that contains it are never found or removed.
  - `exclude = ["~/.config/Foo/keep", "~/.cache/*-tmp"]`: glob patterns (`*` matches within one path segment, `**` across segments). A path that matches, everything under it, and every folder that contains the pattern's fixed part (the text up to the last `/` before the first `*`, or the whole pattern without wildcards) are never found or removed. So excluding `~/.config/Foo/keep` keeps `~/.config/Foo` as well. `scan` and `clean` obey both keys too: an excluded item is never listed, and neither is a folder that a pattern could match something below.
- **Everything is logged and reversible.** Each removal is recorded in `history`; `crumbs restore` puts trashed items back, and files crumbs edited come back from a backup kept next to them.

## Docs

- [JSON output schema](docs/json-schema.md)
- [Design](docs/superpowers/specs/2026-11-08-crumbs-design.md)
