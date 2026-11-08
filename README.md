# crumBS

crumBS is a Linux command-line tool that lists the apps installed on your system across pacman, Flatpak, AppImage and unmanaged `.desktop` sources, finds the files they leave behind, finds space you can reclaim (caches, the trash, old temporary files, logs, superseded AppImages), and moves the items you pick to the trash. It is pre-alpha software.

**Status:** M3: `scan` and `clean` for reclaimable space in your own files, on top of M2's leftovers, orphans, uninstall, history and restore. In this version `uninstall` fully removes only AppImages; for pacman, Flatpak and unmanaged apps it removes just their leftovers (`--leftovers-only`). Package removal needs privilege support and comes later. System-wide leftovers (`/etc`, `/opt`) and system-wide cleanup (`--system`) are not available yet.

## Build

```sh
cargo build --release
./target/release/crumbs --help
```

## Commands

- `crumbs apps list [--source <source>] [--all] [--sort name|size] [--json]` lists installed apps.
- `crumbs apps show <app> [--show-low] [--json]` shows one app and its leftovers.
- `crumbs uninstall <app>... [--leftovers-only] [--keep-data] [--show-low] [-i] [--dry-run] [--yes] [--permanent] [--force] [--json]` removes an app and its leftovers. With `--leftovers-only` the app stays, and so do its launchers, icons and file-type associations. `--keep-data` keeps the app's settings and data and removes only its caches; when the AppImage file itself goes, its own launchers and icons go with it, so nothing is left pointing at a removed file.
- `crumbs orphans [--show-low] [--json]` lists per-user files that belong to no installed app.
- `crumbs orphans --clean (--select <path>... | -i) [--dry-run] [--yes] ...` removes chosen orphans. Nothing is removed unless you name it.
- `crumbs history [--limit N] [--json]` lists past removals, newest first, each batch labelled with the app it uninstalled, `orphans`, `clean` or `restore`.
- `crumbs restore <batch | entry | path>... [--dry-run] [--json]` puts items back. Restoring a batch skips what was permanently deleted or run as a command; naming such an entry by its ID says why it cannot come back.
- `crumbs scan [--category <name>]... [--details] [--json]` lists reclaimable space by category. It only reads: it changes nothing and writes no history. With `--details` every item is listed. Naming categories scans exactly those, even ones switched off in the config.
- `crumbs clean [<category>...] [-i] [--dry-run] [--yes] [--permanent] [--force] [--json]` frees the space `scan` found. With no category it cleans every enabled category; naming categories cleans exactly those. `-i` opens a checklist over all items.
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

Both must be at least 1; a smaller value is replaced by 1 with a warning (`config.toml: temp_age_days must be at least 1; using 1`). A `..` in a `protect`, `exclude` or `appimage_dirs` entry is resolved as the system would: `/../x` is `/x`.

Exit codes: 0 ok, 1 error, 2 usage error or ambiguous app, 3 partial failure, 4 aborted by the user, 5 privilege unavailable.

## Safety

- **Trash first.** Files go to the freedesktop trash, not the void. Only `--permanent` deletes, and it says so in the preview. The exceptions are emptying the trash and clearing `/tmp` (`clean trash`, `clean temp`), which are always permanent and say so.
- **Careful with `/tmp`.** `temp` only runs when `/tmp` is owned by root and has the sticky bit. Only your own files and folders directly in `/tmp` are considered, only when older than `temp_age_days`, and never sockets, pipes, devices, symlinks, names that look like live sessions (X, tmux, ssh, dbus, ...) or anything a running process has open. Each is checked again right before it is deleted, and `temp` refuses to run as root.
- **Preview before anything happens.** `uninstall`, `orphans --clean` and `clean` show every path with its size and the reason it was chosen, then ask. `--dry-run` shows the plan and stops.
- **No surprises in scripts.** Without a terminal, `--yes` is required (or `--dry-run`); `--json` needs one of the two as well.
- **Protected paths.** crumbs refuses system locations, your home directory itself and other paths that should never be removed, re-checks every path right before acting, and skips anything that changed since the scan. Two keys in `~/.config/crumbs/config.toml` add your own (paths start with `/` or `~/`):
  - `protect = ["~/.config/keepme"]`: that path, everything under it and every folder that contains it are never found or removed.
  - `exclude = ["~/.config/Foo/keep", "~/.cache/*-tmp"]`: glob patterns (`*` matches within one path segment, `**` across segments). A path that matches, everything under it, and every folder that contains the pattern's fixed part (the text up to the last `/` before the first `*`, or the whole pattern without wildcards) are never found or removed. So excluding `~/.config/Foo/keep` keeps `~/.config/Foo` as well. `scan` and `clean` obey both keys too: an excluded item is never listed, and neither is a folder that a pattern could match something below.
- **Everything is logged and reversible.** Each removal is recorded in `history`; `crumbs restore` puts trashed items back, and files crumbs edited come back from a backup kept next to them.

## Docs

- [JSON output schema](docs/json-schema.md)
- [Design](docs/superpowers/specs/2026-11-08-crumbs-design.md)
