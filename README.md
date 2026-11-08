# crumBS

crumBS is a Linux command-line tool that lists the apps installed on your system across pacman, Flatpak, AppImage and unmanaged `.desktop` sources, finds the files they leave behind, and moves the ones you pick to the trash. It is pre-alpha software.

**Status:** M2: leftovers, orphans, uninstall, history and restore. In this version `uninstall` fully removes only AppImages; for pacman, Flatpak and unmanaged apps it removes just their leftovers (`--leftovers-only`). Package removal needs privilege support and comes later. System-wide leftovers (`/etc`, `/opt`) are not searched yet.

## Build

```sh
cargo build --release
./target/release/crumbs --help
```

## Commands

- `crumbs apps list [--source <source>] [--all] [--sort name|size] [--json]` lists installed apps.
- `crumbs apps show <app> [--show-low] [--json]` shows one app and its leftovers.
- `crumbs uninstall <app>... [--leftovers-only] [--keep-data] [--show-low] [-i] [--dry-run] [--yes] [--permanent] [--force] [--json]` removes an app and its leftovers. With `--leftovers-only` the app stays, and so do its launchers, icons and file-type associations.
- `crumbs orphans [--show-low] [--json]` lists per-user files that belong to no installed app.
- `crumbs orphans --clean (--select <path>... | -i) [--dry-run] [--yes] ...` removes chosen orphans. Nothing is removed unless you name it.
- `crumbs history [--limit N] [--json]` lists past removals, newest first.
- `crumbs restore <batch | entry | path>... [--dry-run] [--json]` puts items back.
- `crumbs doctor [--json]` reports which sources and tools crumbs can use.

Exit codes: 0 ok, 1 error, 2 usage error or ambiguous app, 3 partial failure, 4 aborted by the user, 5 privilege unavailable.

## Safety

- **Trash first.** Files go to the freedesktop trash, not the void. Only `--permanent` deletes, and it says so in the preview.
- **Preview before anything happens.** `uninstall` and `orphans --clean` show every path with its size and the reason it was chosen, then ask. `--dry-run` shows the plan and stops.
- **No surprises in scripts.** Without a terminal, `--yes` is required (or `--dry-run`); `--json` needs one of the two as well.
- **Protected paths.** crumbs refuses system locations, your home directory itself and other paths that should never be removed, re-checks every path right before acting, and skips anything that changed since the scan. Two keys in `~/.config/crumbs/config.toml` add your own (paths start with `/` or `~/`):
  - `protect = ["~/.config/keepme"]`: that path, everything under it and every folder that contains it are never found or removed.
  - `exclude = ["~/.config/Foo/keep", "~/.cache/*-tmp"]`: glob patterns (`*` matches within one path segment, `**` across segments). A path that matches, everything under it, and every folder that contains the pattern's fixed part (the text up to the last `/` before the first `*`, or the whole pattern without wildcards) are never found or removed. So excluding `~/.config/Foo/keep` keeps `~/.config/Foo` as well.
- **Everything is logged and reversible.** Each removal is recorded in `history`; `crumbs restore` puts trashed items back, and files crumbs edited come back from a backup kept next to them.

## Docs

- [JSON output schema](docs/json-schema.md)
- [Design](docs/superpowers/specs/2026-11-08-crumbs-design.md)
