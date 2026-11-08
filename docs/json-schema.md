# crumBS JSON output (schema version 1)

Every command that takes `--json` prints exactly one JSON document to stdout. Human-readable messages (`error: ...`, `warning: ...`) go to stderr and never mix with it.

All examples below were produced by the debug build against a throwaway fixture root, then trimmed. A line `// ...` inside an example marks removed array items; it is not part of the output. Paths in examples are in-system paths (`/home/user/...` is the fixture's home).

## Stability

Within `schema_version` 1, fields may be **added**, never removed or renamed, and the meaning of an existing field does not change. Readers must ignore fields they do not know. Enum-like strings (`source`, `kind`, `confidence`, `action.type`, ...) may gain new values.

## Envelope

```json
{
  "schema_version": 1,
  "command": "apps show",
  "data": { },
  "errors": []
}
```

| Field | Type | Meaning |
|---|---|---|
| `schema_version` | number | Always `1`. |
| `command` | string | `apps list`, `apps show`, `doctor`, `orphans`, `uninstall`, `history`, `restore`, `scan` or `clean`. |
| `data` | object | The command's payload, described per command below. |
| `errors` | array | Non-fatal problems and refusals. Entries are `{ "source": string, "message": string }`. |

`errors[].source` is one of:

- a source name: `pacman`, `flatpak`, `appimage`, `unmanaged` (that source failed; the others still ran);
- a category name (`scan` and `clean` only): that category's scanner failed, for example `flatpak-unused`;
- `config` (a `config.toml` entry was dropped; the config file is never fatal because of this);
- `rules` (a leftover rules file could not be used);
- `history` (`history` only: unreadable lines of `history.jsonl` were skipped);
- `cli` (the command refused or failed; see below).

### Error envelope

When `uninstall`, `orphans --clean` or `clean` is run with `--json` and refuses or fails (bad flags, unknown or inexact app, source errors without `--force`, a running app, a user "no", ...), it still prints a complete envelope on stdout, with an empty plan, a null report, and one `cli` entry whose `message` is the same text printed to stderr after `error: `:

```json
{
  "schema_version": 1,
  "command": "uninstall",
  "data": {
    "plans": [],
    "report": null
  },
  "errors": [
    {
      "source": "cli",
      "message": "removing this unmanaged app's files isn't possible: /opt/google does not match the app's name; use --leftovers-only"
    }
  ]
}
```

The exit code tells you which kind of failure it was (see below). `apps show` reports "no match" and "ambiguous" through its own `data` shape instead (below). A failure that stops a command before it can produce its result (an unparseable `config.toml`, an unreadable state directory) exits 1 with the message on stderr, and under `--json` also an envelope on stdout with `data: null` and one `errors` entry: `{ "source": "config", "message": ... }` for the config file, `{ "source": "cli", ... }` otherwise.

## Exit codes

| Code | Meaning |
|---|---|
| 0 | Success (including a dry run and "nothing to do"). |
| 1 | Error, or `uninstall`/`orphans --clean`/`clean` refused because a source or scanner failed (override with `--force`) or an app is running, or `uninstall` refused an AppImage whose file fails the safety checks, or refused a package removal: pacman's simulation refused it (for example it would break a dependency), the pacman database is locked (`pacman is busy (database lock present)`), or the name is not a plain package name. |
| 2 | Usage error: bad flags, a missing package tool (`pacman is not installed` when removing a pacman app), an unknown category name, `--user` together with `--system`, a named category of the other kind (`clean cache --system`, `scan --user --category journal`), ambiguous or not-found app, `--json` without `--yes` or `--dry-run`, a destructive command run non-interactively without `--yes`, an unmanaged app whose own files cannot be removed (the refusal text ends `; use --leftovers-only`, see `uninstall`), or `restore` with no selector matching any history entry (see `restore`). |
| 3 | Partial failure: some source failed while another worked, some actions/restores failed (`restore` also when selectors are mixed: a batch with nothing restorable together with an unknown selector) (privileged ones included, when administrator rights were granted but a step failed, or the privileged run stopped part-way), or (`scan`) some category's scanner failed. |
| 4 | Aborted by the user at a prompt. |
| 5 | Administrator rights were needed and not available: no escalation program (`run0` on systemd 256 or newer, `pkexec`, or `sudo` on a terminal) was found, or the prompt was cancelled or denied. Nothing that needs those rights was changed. The human output lists the commands to run by hand, and `--json` carries them in `data.manual_commands` (see `uninstall`). |

## App IDs

An app ID is unique and stable across runs.

| Source | Format | Example |
|---|---|---|
| pacman | `pacman:<package>` | `pacman:firefox` |
| flatpak | `flatpak:user/<app-id>` or `flatpak:system/<app-id>` | `flatpak:user/org.mozilla.firefox` |
| appimage | `appimage:<display path>`; `~/...` under the home, absolute elsewhere | `appimage:~/Applications/GutCheck-0.19.0-x86_64.AppImage` |
| unmanaged | `unmanaged:<desktop id>` | `unmanaged:foo.desktop` |

Commands that take an app accept an ID, a name, a desktop ID or a display name. `uninstall` accepts only exact matches.

## Shared types

**App**

| Field | Type |
|---|---|
| `id` | string |
| `name` | string |
| `display_name` | string |
| `version` | string or null |
| `description` | string or null |
| `source` | `pacman`, `flatpak`, `appimage` or `unmanaged` |
| `installation` | `user`, `system`, or `custom:<name>` for a named Flatpak installation (for example `custom:extra`) |
| `install_size` | number (bytes) or null |
| `location` | string (in-system path) or null |
| `desktop_ids` | array of string |
| `rdns_id` | string or null |
| `exec_names` | array of string |
| `wm_classes` | array of string |
| `has_gui` | boolean |

**Finding** (a leftover or an orphan)

| Field | Type | Meaning |
|---|---|---|
| `path` | string | In-system path. |
| `kind` | string | `config`, `cache`, `data`, `state`, `dot-dir` (a directory directly in home), `dot-file` (a file or symlink there), `flatpak-data`, `snap-data`, `autostart`, `desktop-entry`, `icon`, `systemd-unit`, `mime-assoc`, `system-config` (an unowned directory or file directly under `/etc`) or `system-data` (an unowned directory or file directly under `/opt`). |
| `confidence` | string | `high`, `medium` or `low`. |
| `shared` | boolean | Another installed app may also use it. |
| `reason` | string | Why crumbs thinks it belongs to the app. |
| `size` | object | `{ "bytes", "files", "errors" }`, all numbers. `errors` counts entries that could not be measured. |
| `preselected` | boolean | Whether `uninstall` includes it by default. Always `false` for orphans. |
| `mime_lines` | array of string | `mime-assoc` only: the exact `mimeapps.list` lines that would be removed. Empty otherwise. |
| `scope` | string | `system-tree` for a `system-config` or `system-data` finding (deleted as root, never trashed; see the plan action's `scope`). Absent for everything else (the home directory). |

**System leftovers** (`system-config`, `system-data`) are the direct children of `/etc` and `/opt` that no installed package owns (a path a package ships, or one a tmpfiles.d file declares, counts as owned; tmpfiles.d is read from `/usr/lib/tmpfiles.d`, `/usr/local/lib/tmpfiles.d`, `/run/tmpfiles.d` and `/etc/tmpfiles.d`, and its path fields are unquoted as systemd does, with a leading `%E` read as `/etc`; a literal path covers the candidate when it is the candidate or lies below it, never when it is an ancestor; a path with glob characters (`*?[`) or `%` specifiers covers it only when the pattern is at least as deep and its leading components each match the candidate's, so it could match the candidate or something inside it (each `%X` matches like `*`, never across `/`); a field that cannot be unquoted covers whatever its literal directory, up to the last `/` before the first special character, may hold) and that belong to a removed or detected app. They are never `high` (at most `medium`) and never `preselected`: `uninstall` plans them only when chosen with `-i`, and a plain run adds the human note `N system leftovers not selected (pick with -i)`. `apps show` lists them too, from the same search as `uninstall` (`low` ones only with `--show-low`), with `scope` `system-tree`; its human table marks them `root`. They are deleted permanently, as root, with `scope` `system-tree`. A picked system leftover never `depends_on` anything, not even the app's own removal: it was chosen by hand and no package owns it, as with `--leftovers-only`, so it is deleted even when the removal fails.

**Plan**

| Field | Type | Meaning |
|---|---|---|
| `app_id` | string or null | `null` for orphans. |
| `actions` | array | Planned actions, in execution order. |
| `total_size` | number | Sum of the actions' `size`. |
| `notes` | array of string | For example `the app itself stays installed (leftovers only)`. |

Each action is `{ "action", "size", "reason" }`, plus two optional fields. `action` is tagged by `type`:

- `trash` and `delete` carry `path`;
- `edit-file` carries `path` and `remove_lines` (array of string; the file is edited and a backup is kept next to it);
- `user-command` carries `program` and `args` (array of string). crumbs resolves `program` in the trusted system directories when it runs it, never through `$PATH`. It has no path and cannot be undone. `uninstall` plans one to remove a per-user Flatpak app; `clean` plans one for `flatpak-unused`.
- `privileged-command` carries `program` and `args`, like `user-command`, but runs as root in the run's single privileged batch (see below). It has no path and cannot be undone. `uninstall` plans one to remove a system or custom-installation Flatpak app; `clean --system` plans them for most system categories.
- `package-remove` carries `source` (`pacman`), `names` and `versions` (arrays of string, aligned: the app's package first, then every package pacman will remove with it), `program` and `args`. It runs as root in the privileged batch, has no path and cannot be undone by crumbs.

`size` is bytes (0 for `edit-file`, and 0 for a command or `package-remove` whose size is unknown; a `privileged-command` from `clean --system` carries the size its scan estimated, such as the package cache). A system-tree `delete` whose size could not be read (nothing counted) also has `size` 0; the human preview shows its size as `-` and the total as `will permanently delete N items (X + unknown)`. The JSON does not mark it.

**Privileged batch.** All `privileged-command` and `package-remove` actions of one run, and each `delete` with `scope` `system`, are joined into one `/bin/sh` script and run once, so there is one authorisation prompt however many there are. The escalation program is `run0` (only on systemd 256 or newer), else `pkexec`, else `sudo` (only when stdin is a terminal). The script sets a fixed `PATH` and clears `LD_PRELOAD` and `LD_LIBRARY_PATH`; each program is resolved in the trusted system directories and every argument is single-quoted. It runs before the rest of the plan. The human preview says `these steps run with administrator rights; you will be asked once` (a `note:` line of the human output only; it is not in the plan's `notes`). A privileged action that `depends_on` another action is skipped. `--dry-run` never escalates.

**System-tree deletes.** A `delete` with `scope` `system-tree` is never trashed and cannot be restored. Its target is a direct child of `/etc` or `/opt`, and the chain of directories from `/` down to it must be owned by root and not writable by others. Right before the batch crumbs refuses it when the target changed since the scan, or with one of these `error` texts (that action fails and nothing is deleted): `pacman is busy (database lock present)`, `a mount point is below it` (anything mounted at or below the target, read from `/proc/self/mountinfo`), `the mount table could not be read` (an unreadable or empty table), `a package owns files in it now` (the pacman database and tmpfiles.d are each read again, once per run, and tmpfiles.d declares nothing it covers), `the package database could not be read` (also when it is incomplete, or tmpfiles.d is unreadable) and `the mount namespace could not be read` (also when no trusted `readlink` is installed). The root step then runs `sh -c <guard> crumbs-rm <user mount namespace> <path>`: the guard exits 97 with `pacman is busy (database lock present); not deleting` when `/var/lib/pacman/db.lck` exists (a dangling symlink counts), and with `mount namespace differs; not deleting` unless the root shell shares the user's mount namespace (read with the trusted `readlink` by its absolute path), and only then `exec rm -rf --one-file-system -- <path>`. Its manual command (exit 5) is `rm -rf --one-file-system -- <path>`; the human output then adds the line `check that nothing is mounted inside these paths before running the rm commands` after the list (not in `manual_commands`). The human preview shows `delete (as root)` and the note `system files are deleted permanently with administrator rights and cannot be restored`; that note is also in the plan's `notes`.

The optional fields are left out when they have no value:

| Field | Type | Meaning |
|---|---|---|
| `scope` | string | Where the path may lie: `user-temp` (a direct child of `/tmp`), `home-trash` (an entry of the home trash) or `system` (a root-owned system file, deleted as root in the privileged batch; only `coredumps` uses it) or `system-tree` (a directory tree or file directly under `/etc` or `/opt`, deleted as root with `rm -rf --one-file-system`; see below). Absent means the home directory, as in M2; always absent on an action without a path. |
| `depends_on` | number | Index of an earlier action in the same plan. If that action did not succeed, this one is skipped. The `.trashinfo` file of a trashed entry depends on the entry itself. |

## `apps list`

`data`: `apps` (array of App), `statuses` (array of `{ source, available, app_count, skipped_reason }`; `skipped_reason` is string or null). `errors` carries source failures and config notes; with `--source`, only failures of the listed sources. Exit 3 on partial failure.

```json
{
  "schema_version": 1,
  "command": "apps list",
  "data": {
    "apps": [
      {
        "description": "Standalone web browser",
        "desktop_ids": [
          "firefox.desktop"
        ],
        "display_name": "Firefox Web Browser",
        "exec_names": [
          "firefox"
        ],
        "has_gui": true,
        "id": "pacman:firefox",
        "install_size": 316982060,
        "installation": "system",
        "location": null,
        "name": "firefox",
        "rdns_id": null,
        "source": "pacman",
        "version": "159.0.1-1",
        "wm_classes": [
          "firefox"
        ]
      }
      // ...
    ],
    "statuses": [
      {
        "app_count": 1,
        "available": true,
        "skipped_reason": null,
        "source": "pacman"
      },
      {
        "app_count": 0,
        "available": false,
        "skipped_reason": "not installed",
        "source": "flatpak"
      }
      // ...
    ]
  },
  "errors": []
}
```

## `apps show`

`data` when exactly one app matches: `app` (App) and `leftovers` (array of Finding, highest confidence first; low-confidence findings only with `--show-low`). Exit 0, or 3 on partial failure.

```json
{
  "schema_version": 1,
  "command": "apps show",
  "data": {
    "app": {
      "description": null,
      "desktop_ids": [
        "appimagekit-gutcheck.desktop"
      ],
      "display_name": "GutCheck",
      "exec_names": [
        "GutCheck-0.19.0-x86_64.AppImage"
      ],
      "has_gui": true,
      "id": "appimage:~/Applications/GutCheck-0.19.0-x86_64.AppImage",
      "install_size": 256,
      "installation": "user",
      "location": "/home/user/Applications/GutCheck-0.19.0-x86_64.AppImage",
      "name": "GutCheck-0.19.0-x86_64.AppImage",
      "rdns_id": null,
      "source": "appimage",
      "version": "0.19.0",
      "wm_classes": []
    },
    "leftovers": [
      {
        "confidence": "high",
        "kind": "cache",
        "mime_lines": [],
        "path": "/home/user/.cache/GutCheck",
        "preselected": true,
        "reason": "named after the app name",
        "shared": false,
        "size": {
          "bytes": 53248,
          "errors": 0,
          "files": 1
        }
      }
      // ...
    ]
  },
  "errors": []
}
```

**Not found or ambiguous** (exit 2): `app` is `null`, `query` is the text given, and `candidates` is an empty array (not found) or an array of Apps (ambiguous). There is no `leftovers` field.

```json
{
  "schema_version": 1,
  "command": "apps show",
  "data": {
    "app": null,
    "candidates": [],
    "query": "nosuchapp"
  },
  "errors": []
}
```

## `doctor`

`data`: `crumbs_version` (string), `distro` (`{ id, id_like, name }`), `sources` (array of `{ source, available, app_count, skipped_reason }`), `escalation` (null, or `{ kind, program, requires_tty }` with `kind` one of `run0`, `pkexec`, `sudo`), `trash` (`{ path, exists, writable }`), `data_dirs` (array of string) and `errors` (array of `{ source, message }`, source failures; the envelope's `errors` also carries config notes). Apps found in the `appimage_dirs` of `config.toml` are counted. Exit 3 on partial failure. The human output ends with `privileged cleanup: available via <kind>`, or `unavailable` when there is no escalation program or only `sudo` without a terminal.

```json
{
  "schema_version": 1,
  "command": "doctor",
  "data": {
    "crumbs_version": "0.1.0",
    "distro": {
      "id": "unknown",
      "id_like": [],
      "name": "Unknown"
    },
    "sources": [
      {
        "source": "pacman",
        "available": true,
        "app_count": 1,
        "skipped_reason": null
      }
      // ...
    ],
    "escalation": null,
    "trash": {
      "path": "/home/user/.local/share/Trash",
      "exists": false,
      "writable": false
    },
    "data_dirs": [
      "/usr/local/share",
      "/usr/share"
    ],
    "errors": []
  },
  "errors": []
}
```

## `orphans`

`data`: `orphans` (array of Finding), sorted by confidence (highest first), then path. Exit 0, or 3 on partial failure.

Orphans are never `high`:

- `medium`: a curated rule names an app that is not installed (`looks like thunderbird data; thunderbird was not detected as installed`), data of a Flatpak that is not installed, or a launcher whose program no longer exists;
- `low`: everything else, and anything modified in the last 30 days (its `reason` ends with `; modified N days ago`). Low findings are listed only with `--show-low`.

`preselected` is always `false`: nothing is removed unless named with `orphans --clean --select <path>` or chosen with `-i`.

```json
{
  "schema_version": 1,
  "command": "orphans",
  "data": {
    "orphans": [
      {
        "confidence": "medium",
        "kind": "dot-dir",
        "mime_lines": [],
        "path": "/home/user/.thunderbird",
        "preselected": false,
        "reason": "looks like thunderbird data; thunderbird was not detected as installed",
        "shared": false,
        "size": {
          "bytes": 12288,
          "errors": 0,
          "files": 1
        }
      },
      {
        "confidence": "low",
        "kind": "config",
        "mime_lines": [],
        "path": "/home/user/.config/FreshTool",
        "preselected": false,
        "reason": "does not match any installed app; modified 0 days ago",
        "shared": false,
        "size": {
          "bytes": 0,
          "errors": 0,
          "files": 0
        }
      }
      // ...
    ]
  },
  "errors": []
}
```

`orphans --clean` prints the same `{ plans, report }` shape as `uninstall` (below), with `command` `orphans`; its single plan has `app_id` null.

### `orphans --system`

`crumbs orphans --system [--show-low] [--json]` lists the directories directly under `/opt` that no installed package owns (no pacman-owned path at or below it, nothing declared in tmpfiles.d) and that no detected app claims. It never lists `/etc`: an orphan there cannot be told from a local edit of a live package's config. The shape is the same as `orphans`; every finding has `kind` `system-data`, `scope` `system-tree`, `preselected` false and `confidence` `low`, and since every finding is low, they are listed without `--show-low`. Without a complete pacman database the list is empty. `--user --system` exits 2. `orphans --system --clean --select <path>...` (or `-i`) deletes the chosen ones as root with the system-tree safeguards above; nothing is removed unless named.

```json
{
  "schema_version": 1,
  "command": "orphans",
  "data": {
    "orphans": [
      {
        "confidence": "low",
        "kind": "system-data",
        "mime_lines": [],
        "path": "/opt/old-tool",
        "preselected": false,
        "reason": "not owned by any package and matched by no app",
        "scope": "system-tree",
        "shared": false,
        "size": {
          "bytes": 4096,
          "errors": 0,
          "files": 1
        }
      }
    ]
  },
  "errors": []
}
```

## `uninstall` (and `orphans --clean`)

`data`: `plans` (array of Plan, one per app, in the order given) and `report`:

- `null` on `--dry-run`, and in every error envelope;
- otherwise an array aligned with `plans`, one entry per plan: `null` for a plan with no actions, else an ExecReport. When nothing at all is to be done, every entry is `null`.

**ExecReport**: `batch` (string, the history batch ID), `freed` (number, bytes), `escalation_failed` (boolean) and `results` (array, one per planned action, same order): `{ action, ok, error, history_id }`. `action` is the planned action's `{ type, path, ... }`; `error` is string or null; `history_id` is the entry ID usable with `restore` (for a command it records the run, and there is nothing to restore).

`freed` is the sum of the `size` of every action that succeeded. As in M2 it counts bytes moved to the trash as well as bytes deleted: a trashed item still takes disk space until the trash is emptied. Read it as "removed from where it was", not as "returned to the filesystem". An action skipped because the action it `depends_on` failed has `ok` false and the `error` text `skipped: <path> was not removed`.

`escalation_failed` is `true` when a privileged step never got administrator rights: no escalation program was available, or the prompt was cancelled or denied. Those steps have `ok` false and the `error` `privilege escalation was cancelled or denied` or `no way to get administrator rights (install systemd ≥ 256, polkit or sudo)`. It is `false` for a step that ran and failed, for an action refused before the batch (for example a system file that changed since the scan), and when the privileged run stopped part-way: some steps came back with a result, then none did (the root shell was killed). Then the step it was in has the `error` `the privileged run stopped during this step; it may have partly run`, each later step `not run: the privileged run stopped`, the exit code is 3 (not 5), there are no `manual_commands`, and the `errors` array ends with a `cli` entry `the privileged run stopped before it finished; the step it was in may have partly run, and the steps after it did not run`. Every step is recorded in the history as it ended.

Exit 0 when every action succeeded, 3 when some failed, and 5 when `escalation_failed` is true in any report (see below).

Dry run:

```json
{
  "schema_version": 1,
  "command": "uninstall",
  "data": {
    "plans": [
      {
        "actions": [
          {
            "action": {
              "path": "/home/user/Applications/GutCheck-0.19.0-x86_64.AppImage",
              "type": "trash"
            },
            "reason": "AppImage file",
            "size": 4096
          }
          // ...
        ],
        "app_id": "appimage:~/Applications/GutCheck-0.19.0-x86_64.AppImage",
        "notes": [],
        "total_size": 81920
      }
    ],
    "report": null
  },
  "errors": []
}
```

With `--yes` (the plan is as above; its `actions` are omitted here):

```json
{
  "schema_version": 1,
  "command": "uninstall",
  "data": {
    "plans": [
      // ...
    ],
    "report": [
      {
        "batch": "1794141800823",
        "freed": 81920,
        "results": [
          {
            "action": {
              "path": "/home/user/Applications/GutCheck-0.19.0-x86_64.AppImage",
              "type": "trash"
            },
            "error": null,
            "history_id": "1794141800823-0",
            "ok": true
          }
          // ...
        ]
      }
    ]
  },
  "errors": []
}
```

### Removing the app itself

How the app itself goes depends on its source:

| Source | Action in the plan |
|---|---|
| appimage | `trash` of the AppImage file. |
| pacman | one `package-remove`. crumbs first simulates `pacman -Rs --print` as the user and plans `pacman -Rn --noconfirm <names>` with the exact package list pacman printed (never `-s`, so nothing outside the preview can go). Dependencies that go with it are in `names` after the app's own package and appear in the human preview as `also removed: ...`. |
| flatpak, user installation | `user-command`: `flatpak uninstall --user -y --noninteractive <id>`. No administrator rights. |
| flatpak, system installation | `privileged-command`: `flatpak uninstall --system -y --noninteractive <id>`. |
| flatpak, custom installation | `privileged-command`: `flatpak uninstall --installation=<name> -y --noninteractive <id>`. |
| unmanaged | `uninstall unmanaged:<id>` removes the app's own files by path when that is unambiguous: a `delete` with `scope` `system-tree` for `/opt/<dir>` its canonical program sits in, or a `trash` of its directory in `~/opt` or `~/Applications`, or of the single program file in `~/bin` or `~/.local/bin`. A symlink to the program in a non-protected home location is trashed with it (it `depends_on` the target); a link in a protected location, or outside home, is left with a note. Otherwise exit 2 with `removing this unmanaged app's files isn't possible: <reason>; use --leftovers-only`. The reasons: `its program is in a protected or shared location` (for example `/usr/local`, or a program with no known path), `<path> belongs to a package` (a package owns something at or below it, or tmpfiles.d declares one), `package ownership is unknown (no pacman database)` and `package ownership is unknown (tmpfiles.d unreadable)` (`/opt` targets only), `<path> is shared with <app>, ...` (another detected app lives there), `<path> does not match the app's name` (a vendor directory such as `/opt/google`; only the app's own names count, never names derived from its `Exec` path), or a safety refusal for the path itself. `--leftovers-only` always works as the fallback. |

`--delete-data` is never passed to Flatpak; `~/.var/app/<id>` is a leftover and is trashed, so it can be restored. The leftovers of a removed pacman or Flatpak app are planned with `depends_on` set to the removal's index: if the removal fails or is denied, they are skipped and stay where they are. Picked system leftovers (`scope` `system-tree`) are the exception: they depend on nothing (see System leftovers above), and the same holds beside an unmanaged app's own target. A package removal is confirmed afterwards by reading the pacman database again: a package still installed makes the action `ok` false with `still installed`.

Refusals (exit 2) for an unmanaged app are listed in the table above. Refusals (exit 1): pacman's simulation fails (its message is shown, for example a dependency that would break), or the pacman database lock `db.lck` exists (`pacman is busy (database lock present)`). Names that are not plain package or application names are refused as well.

One run with several apps (`uninstall a b`) still shows one prompt: all privileged actions of all plans share the batch, and each plan gets its own report.

Dry run of `crumbs uninstall firefox` (pacman, trimmed; the fixture's only dependents are `gn` and `libjuice`):

```json
{
  "schema_version": 1,
  "command": "uninstall",
  "data": {
    "plans": [
      {
        "actions": [
          {
            "action": {
              "args": ["-Rn", "--noconfirm", "firefox", "gn", "libjuice"],
              "names": ["firefox", "gn", "libjuice"],
              "program": "pacman",
              "source": "pacman",
              "type": "package-remove",
              "versions": ["159.0.1-1", "0.2385.1-1", "1.7.3-1"]
            },
            "reason": "the app itself",
            "size": 0
          },
          {
            "action": {
              "path": "/home/user/.cache/firefox",
              "type": "trash"
            },
            "depends_on": 0,
            "reason": "named after the app name",
            "size": 4096
          }
        ],
        "app_id": "pacman:firefox",
        "notes": [
          "this cannot be undone by crumbs"
        ],
        "total_size": 4096
      }
    ],
    "report": null
  },
  "errors": []
}
```

The human preview of the same plan:

```
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

A system Flatpak app plans (`flatpak uninstall --system ...`):

```json
{
  "action": {
    "args": ["uninstall", "--system", "-y", "--noninteractive", "com.dosbox_x.DOSBox-X"],
    "program": "flatpak",
    "type": "privileged-command"
  },
  "reason": "the app itself",
  "size": 0
}
```

### Exit 5 and manual commands

Exit 5 has two paths. In both, nothing that needs administrator rights is changed and the commands to run by hand are listed.

**(a) No escalation program.** None of `run0` (systemd 256 or newer), `pkexec` or a terminal `sudo` is usable. crumbs stops after the preview, before any prompt: nothing is run, no report is produced and no history is written. stderr:

```
error: no way to get administrator rights
to do it yourself, run:
  sudo '/usr/bin/pacman' '-Rn' '--noconfirm' 'firefox' 'gn' 'libjuice'
```

With `--json`, `data` is `{ "manual_commands": [...], "plans": [...], "report": null }` and the `errors` array ends with a `cli` entry whose `message` is `no way to get administrator rights`. Every privileged action of the plan is listed (a dependent leftover action is not).

**(b) Prompt cancelled or denied.** An escalation program exists and ran, but no step came back with a result (when some did, the run stopped part-way: exit 3, see `escalation_failed` above). The actions that depend on the privileged ones are skipped and the reports have `escalation_failed` true; the attempt is recorded in the history. The human output prints, after the report, the commands to run by hand:

```
error: administrator rights were not granted; nothing that needs them was changed
to do it yourself, run:
  sudo '/usr/bin/pacman' '-Rn' '--noconfirm' 'firefox' 'gn' 'libjuice'
```

With `--json` the envelope has `data.manual_commands`: an array of those lines, one per privileged action that never got rights, in plan order, each exactly as the batch would run it (program path, every argument single-quoted). It is present only in an exit-5 envelope, for both paths. A dependent that was skipped, and an action crumbs refused before the batch (it changed since the scan, or failed a safety check), are never listed: running them by hand would skip crumbs' checks. In path (b) the `errors` array ends with a `cli` entry carrying the same text as the `error:` line, and `data.report` holds the reports, with `escalation_failed` true. The attempt is recorded in the history with `ok` false. The example below is path (b).

```json
{
  "schema_version": 1,
  "command": "uninstall",
  "data": {
    "manual_commands": [
      "sudo '/usr/bin/pacman' '-Rn' '--noconfirm' 'firefox' 'gn' 'libjuice'"
    ],
    "plans": [
      // ...
    ],
    "report": [
      {
        "batch": "1794164328456",
        "escalation_failed": true,
        "freed": 0,
        "results": [
          {
            "action": {
              "args": ["-Rn", "--noconfirm", "firefox", "gn", "libjuice"],
              "names": ["firefox", "gn", "libjuice"],
              "program": "pacman",
              "source": "pacman",
              "type": "package-remove",
              "versions": ["159.0.1-1", "0.2385.1-1", "1.7.3-1"]
            },
            "error": "privilege escalation was cancelled or denied; still installed: firefox, gn, libjuice",
            "history_id": "1794164328456-0",
            "ok": false
          },
          {
            "action": {
              "path": "/home/user/.cache/firefox",
              "type": "trash"
            },
            "error": "skipped: pacman was not removed",
            "history_id": "1794164328456-1",
            "ok": false
          }
        ]
      }
    ]
  },
  "errors": [
    {
      "source": "cli",
      "message": "administrator rights were not granted; nothing that needs them was changed"
    }
  ]
}
```

An AppImage whose file fails the safety checks (outside home, protected or excluded, or under a symlinked directory) is refused before anything is planned: exit 1 with `cannot remove <id>: <reason>; use --leftovers-only to remove only its leftovers`.

With `--leftovers-only`, the plan leaves out what launches or opens the app: its launchers (named by its desktop IDs, or running its AppImage file), icons named after them, and `mimeapps.list` associations.

## `history`

`data`: `batches`, newest first (`--limit N`, default 20). Each batch:

| Field | Type | Meaning |
|---|---|---|
| `batch` | string | Batch ID. |
| `time` | string | RFC 3339 UTC time of the batch's first entry. |
| `label` | string | By the entries' `kind`: the app ID for `uninstall`, otherwise `orphans` or `clean`; `restore` for a batch made by `restore`. Entries without `kind` (written by earlier builds) give the app ID, or `orphans` when there is none. |
| `items` | number | Number of entries. |
| `size` | number | Bytes of the entries that succeeded. |
| `restored` | number | Entries of this batch that a later successful restore put back. |
| `entries` | array | History entries (below). |

**History entry**: `id` (`<batch>-<index>`), `batch`, `time`, `action` (`trash`, `delete`, `edit-file`, `restore`, `command`, `privileged-command` or `package-remove`; lines written by earlier builds may say `edit_file`, which means the same), `path`, `trashed` (null, or `{ original, files_path, info_path }`), `backup` (string or null), `size` (number), `ok` (boolean), `error` (string or null), `app_id` (string or null), `restores` (string or null), `argv` (array of string; present on `command`, `privileged-command` and `package-remove` entries only), `packages` (array of `[name, version]` pairs; present on `package-remove` entries only) and `scope` (string; present only on a system delete: `system` or `system-tree`) and `kind` (string: `uninstall`, `orphans` or `clean`, the command that made the batch; absent on `restore` entries and on lines written by earlier builds).

- `trashed` is set for `trash` entries, and on `restore` entries it repeats the trash record that was restored.
- On an `edit-file` entry, `backup` is the copy of the file as it was before the edit.
- On a `restore` entry, `restores` is the ID of the entry it restored. For a restored `edit-file`, `backup` is the **pre-restore backup**: a copy of the file as it was just before the restore overwrote it. For other entries `backup` is null.
- On a `command` entry (a `user-command` action that ran), `path` is the program's in-system path (or its bare name when it is not installed), `argv` its arguments, `size` 0, `trashed` and `backup` null. It cannot be restored.
- A `privileged-command` entry is the same for a command that ran as root: `path` is the program's in-system path, `argv` its arguments, `size` the size the scan estimated (0 when unknown), `trashed` and `backup` null. It cannot be restored.
- A `package-remove` entry records a package removal that ran as root: `path` is the package manager (`/usr/bin/pacman`), `argv` the exact arguments, `packages` the `[name, version]` of each package that is **gone** afterwards (empty when the removal failed or was denied; after a partial failure only what was removed). `crumbs restore` says `packages cannot be restored by crumbs`. A permanent `delete` of a system file (`coredumps`) is recorded as a `delete` entry, and so is a system-tree delete, with `scope` `system-tree`; both are root-owned deletes that `restore` refuses by name.
- Failed and skipped attempts are recorded too (`ok` false, `error` set). A denied escalation is recorded with `ok` false.
- A batch made by `clean` has `app_id` null and `kind` `clean` on its entries, so `history` labels it `clean`.

The history file is `history.jsonl` in the `crumbs` directory under the state directory (`~/.local/state/crumbs`). crumbs opens the directory and the file without following symlinks and refuses (an error, nothing written) when either is a symlink. An existing directory or file with looser permissions is tightened to `0700` and `0600` (owner bits are kept), and each tightening is reported as `warning: history: ...` on stderr; a symlinked directory is neither followed nor tightened.

`errors` has a `history` entry when unreadable lines were skipped. Exit 0. Example (a trashed orphan, then a restore of it; one batch of each):

```json
{
  "schema_version": 1,
  "command": "history",
  "data": {
    "batches": [
      {
        "batch": "1794141806566",
        "entries": [
          {
            "action": "restore",
            "app_id": null,
            "backup": null,
            "batch": "1794141806566",
            "error": null,
            "id": "1794141806566-0",
            "ok": true,
            "path": "/home/user/.thunderbird",
            "restores": "1794141800816-0",
            "size": 12288,
            "time": "2026-11-08T12:43:26.566420307Z",
            "trashed": {
              "files_path": "/home/user/.local/share/Trash/files/.thunderbird",
              "info_path": "/home/user/.local/share/Trash/info/.thunderbird.trashinfo",
              "original": "/home/user/.thunderbird"
            }
          }
        ],
        "items": 1,
        "label": "restore",
        "restored": 0,
        "size": 12288,
        "time": "2026-11-08T12:43:26.566420307Z"
      },
      {
        "batch": "1794141800816",
        "entries": [
          {
            "action": "trash",
            "app_id": null,
            "backup": null,
            "batch": "1794141800816",
            "error": null,
            "id": "1794141800816-0",
            "kind": "orphans",
            "ok": true,
            "path": "/home/user/.thunderbird",
            "restores": null,
            "size": 12288,
            "time": "2026-11-08T12:43:20.820096179Z",
            "trashed": {
              "files_path": "/home/user/.local/share/Trash/files/.thunderbird",
              "info_path": "/home/user/.local/share/Trash/info/.thunderbird.trashinfo",
              "original": "/home/user/.thunderbird"
            }
          }
        ],
        "items": 1,
        "label": "orphans",
        "restored": 1,
        "size": 12288,
        "time": "2026-11-08T12:43:20.820096179Z"
      }
    ]
  },
  "errors": []
}
```

The history itself is the append-only file `{state_home}/crumbs/history.jsonl`, one entry object per line.

## `restore`

Selectors are batch IDs, entry IDs or absolute original paths. `data`: `outcomes`, one per matched entry, plus one per selector that matched nothing:

| Field | Type | Meaning |
|---|---|---|
| `entry_id` | string | The restored history entry; empty string when the selector matched nothing, and also for a recognised batch with nothing restorable (below). |
| `path` | string | The entry's original path (the selector text when nothing matched). |
| `ok` | boolean | |
| `error` | string or null | For example `already restored`, `no matching history entry`. On an `ok` outcome with an empty `entry_id` it is informational (below). |

A batch selector restores the batch's successful `trash` and `edit-file` entries; `delete`, `command`, `privileged-command` and `package-remove` entries are skipped, since nothing of them can be put back (an entry ID names one anyway, and its outcome says `permanently deleted; cannot be restored`, `commands cannot be undone` or `packages cannot be restored by crumbs; reinstall with: pacman -S <app> && pacman -S --asdeps <deps>`, where the app is the one `app_id` names and the rest were removed with it; a removal that failed part-way still gives the commands for what it removed, prefixed with `the original removal only partly ran; `). A path selector names the newest successful trash entry for that path that has not been restored yet. `--dry-run` checks and reports without changing anything. A batch that exists but holds nothing that can be put back (it only deleted, ran commands or removed packages, or every trash entry in it failed) is not an error: its outcome is `ok: true`, `entry_id` empty, `path` the selector and `error` the informational text `nothing in this batch can be restored`. Consumers should key on `ok` together with `entry_id`: `ok` true with an empty `entry_id` means a recognised batch with nothing to restore, `ok` false with an empty `entry_id` means the selector matched nothing (`no matching history entry`).

Exit 0 when every outcome is `ok`; 2 when every outcome has an empty `entry_id` and none is `ok` (no selector matched any history entry); otherwise 3. So a nothing-restorable batch alone exits 0, and one together with an unknown selector exits 3.

Two further refusal texts, both with `ok` false and the entry's own `entry_id`:

- A system-tree delete (`uninstall` of an unmanaged `/opt` app, `orphans --system --clean`, and any other `delete` with `scope` `system-tree`): `system files deleted as root cannot be restored`, also when that delete failed.
- A trash entry whose trash directory fails the private-directory rule: `trash directory <path> has unsafe permissions or owner (needs a private 0700 directory owned by you)`. The trash root must be a real directory, owned by you, mode exactly `0700` and, for the home trash, inside your home directory; the same rule decides where `clean`, `uninstall` and `orphans` may trash to, so a refused root is skipped at trashing time too.

```json
{
  "schema_version": 1,
  "command": "restore",
  "data": {
    "outcomes": [
      {
        "entry_id": "1794141800816-0",
        "error": "already restored",
        "ok": false,
        "path": "/home/user/.thunderbird"
      },
      {
        "entry_id": "",
        "error": "no matching history entry",
        "ok": false,
        "path": "/home/user/nothing"
      }
    ]
  },
  "errors": []
}
```

## `scan`

`crumbs scan [--category <name>]... [--details] [--user] [--json]` is strictly read-only: it never changes a file and never writes history. `--system` scans the system categories instead (also read-only, no administrator rights needed; naming a system category with `--category` implies it). `--user --system` exits 2 with `use either --user or --system`, and so does a named category of the other kind (`category 'cache' is a user category and cannot be used with --system`); under `--json` that is an envelope with `data: null` and a `cli` error. Under `--json` a category that cannot run here has an `unavailable` reason; the human table shows `unavailable (<reason>)` in its note column.

`data`: `categories`, an array of **CategoryReport**, one per listed category in this fixed order. The nine user categories: `cache`, `thumbnails`, `trash`, `temp`, `browser-cache`, `dev-cache`, `flatpak-unused`, `appimage-old`, `logs`. The six system categories, after them: `pkg-cache`, `orphan-pkgs`, `journal`, `coredumps`, `system-temp`, `flatpak-unused-system`.

- With no `--category`, the nine user categories are listed; with `--system`, the six system ones. Categories switched off in `categories_off` (default: `dev-cache`) are listed with `skipped: true`, an empty `items` and `total` 0, and are not scanned.
- With `--category`, exactly the named categories are listed and scanned, even ones switched off in the config.

| Field | Type | Meaning |
|---|---|---|
| `category` | string | One of the fifteen names above. |
| `items` | array | BloatItems, largest first. |
| `total` | number | Sum of the known item sizes, in bytes. |
| `unknown_sizes` | number | Items whose size is unknown (`size` null). The real total is larger. |
| `error` | string or null | Set when this category's scanner failed. `items` is then empty. |
| `skipped` | boolean | `true` for a category that is switched off and was not named: it was not scanned. |
| `unavailable` | string | Present only when the category cannot run on this system (for example `pacman not installed`). `items` is then empty and `error` null; this is not a failure and does not change the exit code. |

**BloatItem**

| Field | Type | Meaning |
|---|---|---|
| `category` | string | The category. |
| `path` | string or null | In-system path. `null` for a command item. |
| `label` | string | What to show: a name, a display path, or for a command what it does. |
| `size` | number or null | Bytes. `null` when unknown. |
| `rebuildable` | boolean | The owner recreates it on demand (caches). |
| `preselected` | boolean | Whether `clean` includes it by default. `false` for an item held back (its app is running, see `note`) and for `flatpak-unused`, `trash`, `orphan-pkgs` and `flatpak-unused-system` items, which `clean` includes only when their category is named. |
| `note` | string or null | Why it is held back (`Firefox is running`), or what to know (`deleted permanently (temporary file)`, `decided by flatpak`). |
| `action` | string or object | `"trash"`, `"delete"`, `{ "command": { "program", "args" } }` (runs as the user) or `{ "privileged": { "program", "args" } }` (runs as root; `program` is a bare name that crumbs resolves in the trusted directories). |
| `scope` | string | `home`, `user-temp`, `home-trash` or `system` (see the plan action's `scope`). Always present here. |
| `extra_paths` | array of string | Further paths removed together with the item, for example the `.trashinfo` file of a trashed entry. Paths only: the safety fingerprints are internal and not shown. |
| `app_id` | string or null | The app the item belongs to, when exactly one installed app matches. |

The internal fingerprint of an item is never serialized.

**System categories.** `scan --system` reads only: the scanners run as the user and nothing is escalated.

| Category | Item | Action | Notes |
|---|---|---|---|
| `pkg-cache` | one item, `package cache (keep N versions)` | `privileged`: `paccache -rkN` | `N` is `pkg_cache_keep` (default 2); the size is `paccache -dkN`'s. Without `paccache` (pacman-contrib), `pacman -Sc --noconfirm` with an unknown size and a note. `unavailable` without pacman. |
| `orphan-pkgs` | one item, `N orphaned packages`, the names in `note` | `privileged`: `pacman -Rn --noconfirm <names>` | Opt-in. The names are the closure of `pacman -Rs --print` over `pacman -Qdtq`, so exactly what was simulated goes. |
| `journal` | one item, `systemd journal` | `privileged`: `journalctl --vacuum-size=<journal_max>` | Listed only when the journal is larger than `journal_max` (default `500M`); the size is the excess. |
| `coredumps` | one item per `core.*` file in `/var/lib/systemd/coredump` | `delete` with `scope` `system` (run as `rm -f -- <path>` in the batch) | Regular files only, with every directory from `/` root-owned and not writable by others. `unavailable` without the directory. |
| `system-temp` | one item, `stale system temporary files` | `privileged`: `systemd-tmpfiles --clean --prefix=/tmp --prefix=/var/tmp` | Size unknown: systemd decides by its age rules. |
| `flatpak-unused-system` | one item, `unused Flatpak runtimes (system)` | `privileged`: `flatpak uninstall --system --unused -y --noninteractive` | Opt-in. Size unknown. |

`errors` has one entry per failed scanner (`source` is the category name), then one per failed app source, then config notes. Exit 0, or 3 when a scanner failed (a failed app source alone does not change the exit code). Example (the fixture has a user Flatpak runtime, an old log, an old file in `/tmp` and two AppImage versions):

```json
{
  "schema_version": 1,
  "command": "scan",
  "data": {
    "categories": [
      {
        "category": "cache",
        "items": [
          {
            "category": "cache",
            "path": "/home/user/.cache/foo",
            "label": "foo",
            "size": 4096,
            "rebuildable": true,
            "preselected": true,
            "note": null,
            "action": "trash",
            "scope": "home",
            "extra_paths": [],
            "app_id": null
          }
        ],
        "total": 4096,
        "unknown_sizes": 0,
        "error": null,
        "skipped": false
      },
      // ...
      {
        "category": "trash",
        "items": [
          {
            "category": "trash",
            "path": "/home/user/.local/share/Trash/files/old.txt",
            "label": "~/docs/old.txt",
            "size": 8192,
            "rebuildable": false,
            "preselected": false,
            "note": null,
            "action": "delete",
            "scope": "home-trash",
            "extra_paths": [
              "/home/user/.local/share/Trash/info/old.txt.trashinfo"
            ],
            "app_id": null
          }
        ],
        "total": 8192,
        "unknown_sizes": 0,
        "error": null,
        "skipped": false
      },
      {
        "category": "temp",
        "items": [
          {
            "category": "temp",
            "path": "/tmp/build-output.bin",
            "label": "/tmp/build-output.bin",
            "size": 8192,
            "rebuildable": false,
            "preselected": true,
            "note": "deleted permanently (temporary file)",
            "action": "delete",
            "scope": "user-temp",
            "extra_paths": [],
            "app_id": null
          }
        ],
        "total": 8192,
        "unknown_sizes": 0,
        "error": null,
        "skipped": false
      },
      // ...
      {
        "category": "dev-cache",
        "items": [],
        "total": 0,
        "unknown_sizes": 0,
        "error": null,
        "skipped": true
      },
      {
        "category": "flatpak-unused",
        "items": [
          {
            "category": "flatpak-unused",
            "path": null,
            "label": "unused Flatpak runtimes (user)",
            "size": null,
            "rebuildable": false,
            "preselected": false,
            "note": "decided by flatpak",
            "action": {
              "command": {
                "program": "flatpak",
                "args": [
                  "uninstall",
                  "--user",
                  "--unused",
                  "--noninteractive",
                  "-y"
                ]
              }
            },
            "scope": "home",
            "extra_paths": [],
            "app_id": null
          }
        ],
        "total": 0,
        "unknown_sizes": 1,
        "error": null,
        "skipped": false
      },
      {
        "category": "appimage-old",
        "items": [
          {
            "category": "appimage-old",
            "path": "/home/user/Applications/Tool-1.0-x86_64.AppImage",
            "label": "Tool-1.0-x86_64.AppImage",
            "size": 4096,
            "rebuildable": false,
            "preselected": true,
            "note": null,
            "action": "trash",
            "scope": "home",
            "extra_paths": [],
            "app_id": "appimage:~/Applications/Tool-1.0-x86_64.AppImage"
          }
        ],
        "total": 4096,
        "unknown_sizes": 0,
        "error": null,
        "skipped": false
      }
      // ...
    ]
  },
  "errors": []
}
```

`scan --system --json` (a fixture where `paccache`, `pacman`, `journalctl`, `systemd-tmpfiles` and `flatpak` are stubs; the first four categories are trimmed to their first items):

```json
{
  "schema_version": 1,
  "command": "scan",
  "data": {
    "categories": [
      {
        "category": "pkg-cache",
        "items": [
          {
            "category": "pkg-cache",
            "path": null,
            "label": "package cache (keep 2 versions)",
            "size": 90355794,
            "rebuildable": true,
            "preselected": true,
            "note": null,
            "action": {
              "privileged": {
                "program": "paccache",
                "args": ["-rk2"]
              }
            },
            "scope": "system",
            "extra_paths": [],
            "app_id": null
          }
        ],
        "total": 90355794,
        "unknown_sizes": 0,
        "error": null,
        "skipped": false
      },
      {
        "category": "orphan-pkgs",
        "items": [
          {
            "category": "orphan-pkgs",
            "path": null,
            "label": "2 orphaned packages",
            "size": 8000000,
            "rebuildable": false,
            "preselected": false,
            "note": "orph1, orph2",
            "action": {
              "privileged": {
                "program": "pacman",
                "args": ["-Rn", "--noconfirm", "orph1", "orph2"]
              }
            },
            "scope": "system",
            "extra_paths": [],
            "app_id": null
          }
        ],
        "total": 8000000,
        "unknown_sizes": 0,
        "error": null,
        "skipped": false
      },
      // journal ...
      {
        "category": "coredumps",
        "items": [
          {
            "category": "coredumps",
            "path": "/var/lib/systemd/coredump/core.foo.1000.abc.1.zst",
            "label": "core.foo.1000.abc.1.zst",
            "size": 12288,
            "rebuildable": false,
            "preselected": true,
            "note": null,
            "action": "delete",
            "scope": "system",
            "extra_paths": [],
            "app_id": null
          }
        ],
        "total": 12288,
        "unknown_sizes": 0,
        "error": null,
        "skipped": false
      },
      // system-temp, flatpak-unused-system ...
    ]
  },
  "errors": []
}
```

On a system without pacman, `crumbs scan --category pkg-cache --json` lists the category as unavailable (not an error, exit 0):

```json
{
  "category": "pkg-cache",
  "items": [],
  "total": 0,
  "unknown_sizes": 0,
  "error": null,
  "skipped": false,
  "unavailable": "pacman not installed"
}
```

and the table says `unavailable (pacman not installed)` in the note column.

When a scanner fails, its category has `error` set and the exit code is 3. Here `flatpak` itself failed, so the Flatpak app source is reported as well:

```json
{
  "category": "flatpak-unused",
  "items": [],
  "total": 0,
  "unknown_sizes": 0,
  "error": "command flatpak failed: boom",
  "skipped": false
}
```

with these envelope `errors`:

```json
[
  { "source": "flatpak-unused", "message": "command flatpak failed: boom" },
  { "source": "flatpak", "message": "boom" }
]
```

## `clean`

`crumbs clean [<category>...] [-i] [--dry-run] [--yes] [--permanent] [--force] [--json] [--user | --system]` prints the same `{ plans, report }` shape as `uninstall`, with `command` `clean`; it always has exactly one plan, with `app_id` null, or none at all when there is nothing to do. `--json` needs `--yes` or `--dry-run`, as for `uninstall`.

- Without `--system`, `clean` cleans the user categories. With `--system` it cleans the system categories instead; naming a system category implies it, so `crumbs clean journal` works.
- `--user --system`, or a named category of the other kind, exits 2 with the same message as `scan` (`use either --user or --system`, `category 'cache' is a user category and cannot be used with --system`) and an error envelope whose `data` is `{ "plans": [], "report": null }`.
- System items become `privileged-command` actions, or (`coredumps`) `delete` actions with `scope` `system`; `package-remove` is planned only by `uninstall`. They run in the one privileged batch described under Plan, so a run asks once. A system `clean` also records `kind` `clean` in the history.
- The system items preselected are those of `pkg-cache`, `journal`, `coredumps` and `system-temp`. `orphan-pkgs` and `flatpak-unused-system` are opt-in and are planned only when named (or ticked with `-i`).
- If the escalation fails, the run exits 5 with `data.manual_commands`, as for `uninstall`.

Which items are planned:

- With no category, every enabled category is scanned (all but those in `categories_off`). Naming categories scans exactly those, even ones switched off in the config.
- The items cleaned are the ones with `preselected` true. An item held back (its app is running) is left out and listed nowhere in the JSON. With `-i` you pick from all items instead.
- `flatpak-unused` and `trash` are opt-in: their items are never preselected, so they are planned only when the category is named (or ticked with `-i`). A plain `crumbs clean` never empties the trash. The same holds for `orphan-pkgs` and `flatpak-unused-system`.
- `trash` and `temp` items are always permanent deletes (`delete` actions with `scope` `home-trash` and `user-temp`, and the plan's `notes` say so). With `--permanent`, trash items of the other categories become `delete` too.
- When any action is a `trash`, the plan's `notes` include `space is freed when the trash is emptied (or use --permanent)`.

Refusals:

- If an app source or a category's scanner failed, `clean` refuses with exit 1 and `refusing: some sources failed (...); re-run with --force to continue`, unless `--force` or `--dry-run` is given (a dry run only warns). The failed sources and scanners lead `errors`, followed by the `cli` entry, and `data` is `{ "plans": [], "report": null }`.
- Nothing chosen (nothing found, or everything held back): exit 0 and `data` is `{ "plans": [], "report": [] }`, or `{ "plans": [], "report": null }` with `--dry-run`.
- Any other refusal or failure uses the error envelope described at the top, with `report` null.

Exit 0 when every action succeeded, 3 when some failed, 5 when administrator rights were needed and not available.

Dry run of `crumbs clean --system` (the fixture of the `scan --system` example without `pacman` and `paccache`; the `notes` are empty because the administrator-rights notice is a line of the human preview only):

```json
{
  "schema_version": 1,
  "command": "clean",
  "data": {
    "plans": [
      {
        "actions": [
          {
            "action": {
              "args": ["--vacuum-size=500M"],
              "program": "journalctl",
              "type": "privileged-command"
            },
            "reason": "vacuums to 500M",
            "size": 229743002
          },
          {
            "action": {
              "path": "/var/lib/systemd/coredump/core.foo.1000.abc.1.zst",
              "type": "delete"
            },
            "reason": "core.foo.1000.abc.1.zst",
            "scope": "system",
            "size": 12288
          },
          {
            "action": {
              "args": ["--clean", "--prefix=/tmp", "--prefix=/var/tmp"],
              "program": "systemd-tmpfiles",
              "type": "privileged-command"
            },
            "reason": "removes files past the tmpfiles.d age limits under /tmp and /var/tmp",
            "size": 0
          }
        ],
        "app_id": null,
        "notes": [],
        "total_size": 229755290
      }
    ],
    "report": null
  },
  "errors": []
}
```

The human preview of that plan shows each privileged command exactly as the batch will run it, then the consent note:

```
Clean
ACTION  SIZE      PATH                                                      REASON
run     —         journalctl --vacuum-size=500M                             vacuums to 500M
    - as root: '/usr/bin/journalctl' '--vacuum-size=500M'
delete  12.0 KiB  /var/lib/systemd/coredump/core.foo.1000.abc.1.zst         core.foo.1000.abc.1.zst
    - as root: '/usr/bin/rm' '-f' '--' '/var/lib/systemd/coredump/core.foo.1000.abc.1.zst'
run     —         systemd-tmpfiles --clean --prefix=/tmp --prefix=/var/tmp  removes files past the tmpfiles.d age limits under /tmp and /var/tmp
    - as root: '/usr/bin/systemd-tmpfiles' '--clean' '--prefix=/tmp' '--prefix=/var/tmp'
note: these steps run with administrator rights; you will be asked once

will permanently delete 1 item (12.0 KiB)
will run 2 commands
```

Denied escalation, `crumbs clean --system --yes --json`, exit 5: `data.manual_commands` is

```json
[
  "sudo '/usr/bin/journalctl' '--vacuum-size=500M'",
  "sudo '/usr/bin/rm' '-f' '--' '/var/lib/systemd/coredump/core.foo.1000.abc.1.zst'",
  "sudo '/usr/bin/systemd-tmpfiles' '--clean' '--prefix=/tmp' '--prefix=/var/tmp'"
]
```

and the report has `escalation_failed` true.

Dry run of `crumbs clean trash temp flatpak-unused`:

```json
{
  "schema_version": 1,
  "command": "clean",
  "data": {
    "plans": [
      {
        "actions": [
          {
            "action": {
              "path": "/home/user/.local/share/Trash/files/old.txt",
              "type": "delete"
            },
            "reason": "~/docs/old.txt",
            "scope": "home-trash",
            "size": 8192
          },
          {
            "action": {
              "path": "/home/user/.local/share/Trash/info/old.txt.trashinfo",
              "type": "delete"
            },
            "depends_on": 0,
            "reason": "~/docs/old.txt",
            "scope": "home-trash",
            "size": 0
          },
          {
            "action": {
              "path": "/tmp/build-output.bin",
              "type": "delete"
            },
            "reason": "deleted permanently (temporary file)",
            "scope": "user-temp",
            "size": 8192
          },
          {
            "action": {
              "args": [
                "uninstall",
                "--user",
                "--unused",
                "--noninteractive",
                "-y"
              ],
              "program": "flatpak",
              "type": "user-command"
            },
            "reason": "decided by flatpak",
            "size": 0
          }
        ],
        "app_id": null,
        "notes": [
          "permanently delete (temporary files)",
          "permanently delete (emptying the trash)"
        ],
        "total_size": 16384
      }
    ],
    "report": null
  },
  "errors": []
}
```

`crumbs clean cache --yes --json` (the plan is shown in full; `freed` counts the trashed bytes, as in M2):

```json
{
  "schema_version": 1,
  "command": "clean",
  "data": {
    "plans": [
      {
        "actions": [
          {
            "action": {
              "path": "/home/user/.cache/foo",
              "type": "trash"
            },
            "reason": "foo",
            "size": 4096
          }
        ],
        "app_id": null,
        "notes": [
          "space is freed when the trash is emptied (or use --permanent)"
        ],
        "total_size": 4096
      }
    ],
    "report": [
      {
        "batch": "1794149571972",
        "freed": 4096,
        "results": [
          {
            "action": {
              "path": "/home/user/.cache/foo",
              "type": "trash"
            },
            "error": null,
            "history_id": "1794149571972-0",
            "ok": true
          }
        ]
      }
    ]
  },
  "errors": []
}
```

Nothing to clean, with `--yes`:

```json
{
  "schema_version": 1,
  "command": "clean",
  "data": {
    "plans": [],
    "report": []
  },
  "errors": []
}
```

Refused because a scanner failed (exit 1):

```json
{
  "schema_version": 1,
  "command": "clean",
  "data": {
    "plans": [],
    "report": null
  },
  "errors": [
    {
      "source": "flatpak",
      "message": "boom"
    },
    {
      "source": "flatpak-unused",
      "message": "command flatpak failed: boom"
    },
    {
      "source": "cli",
      "message": "refusing: some sources failed (flatpak, flatpak-unused); re-run with --force to continue"
    }
  ]
}
```

The history entry that running the Flatpak command leaves (see `history`):

```json
{
  "action": "command",
  "app_id": null,
  "argv": [
    "uninstall",
    "--user",
    "--unused",
    "--noninteractive",
    "-y"
  ],
  "backup": null,
  "batch": "1794149571982",
  "error": null,
  "id": "1794149571982-0",
  "ok": true,
  "path": "/usr/bin/flatpak",
  "restores": null,
  "size": 0,
  "time": "2026-11-08T14:52:51.98359927Z",
  "trashed": null
}
```
