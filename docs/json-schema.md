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
      "message": "removing pacman apps needs privileged support (coming in a later version); use --leftovers-only"
    }
  ]
}
```

The exit code tells you which kind of failure it was (see below). `apps show` reports "no match" and "ambiguous" through its own `data` shape instead (below). A failure that stops a command before it can produce its result (an unparseable `config.toml`, an unreadable state directory) exits 1 with the message on stderr, and under `--json` also an envelope on stdout with `data: null` and one `errors` entry: `{ "source": "config", "message": ... }` for the config file, `{ "source": "cli", ... }` otherwise.

## Exit codes

| Code | Meaning |
|---|---|
| 0 | Success (including a dry run and "nothing to do"). |
| 1 | Error, or `uninstall`/`orphans --clean`/`clean` refused because a source or scanner failed (override with `--force`) or an app is running, or `uninstall` refused an AppImage whose file fails the safety checks. |
| 2 | Usage error: bad flags, an unknown category name, `--system` (not available yet), ambiguous or not-found app, `--json` without `--yes` or `--dry-run`, a destructive command run non-interactively without `--yes`, removing a non-AppImage app without `--leftovers-only`, or `restore` with no selector matching any history entry. |
| 3 | Partial failure: some source failed while another worked, some actions/restores failed, or (`scan`) some category's scanner failed. |
| 4 | Aborted by the user at a prompt. |
| 5 | Privilege escalation unavailable (reserved; not returned by M2 or M3 commands). |

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
| `installation` | `user` or `system` |
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
| `kind` | string | `config`, `cache`, `data`, `state`, `dot-dir` (a directory directly in home), `dot-file` (a file or symlink there), `flatpak-data`, `snap-data`, `autostart`, `desktop-entry`, `icon`, `systemd-unit` or `mime-assoc`. |
| `confidence` | string | `high`, `medium` or `low`. |
| `shared` | boolean | Another installed app may also use it. |
| `reason` | string | Why crumbs thinks it belongs to the app. |
| `size` | object | `{ "bytes", "files", "errors" }`, all numbers. `errors` counts entries that could not be measured. |
| `preselected` | boolean | Whether `uninstall` includes it by default. Always `false` for orphans. |
| `mime_lines` | array of string | `mime-assoc` only: the exact `mimeapps.list` lines that would be removed. Empty otherwise. |

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
- `user-command` (`clean` only) carries `program` and `args` (array of string). crumbs resolves `program` in the trusted system directories when it runs it, never through `$PATH`. It has no path and cannot be undone.

`size` is bytes (0 for `edit-file`, and 0 for `user-command`, whose size is unknown).

The optional fields are left out when they have no value:

| Field | Type | Meaning |
|---|---|---|
| `scope` | string | Where the path may lie: `user-temp` (a direct child of `/tmp`) or `home-trash` (an entry of the home trash). Absent means the home directory, as in M2. |
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

`data`: `crumbs_version` (string), `distro` (`{ id, id_like, name }`), `sources` (array of `{ source, available, app_count, skipped_reason }`), `escalation` (null, or `{ kind, program, requires_tty }` with `kind` one of `run0`, `pkexec`, `sudo`), `trash` (`{ path, exists, writable }`), `data_dirs` (array of string) and `errors` (array of `{ source, message }`, source failures; the envelope's `errors` also carries config notes). Apps found in the `appimage_dirs` of `config.toml` are counted. Exit 3 on partial failure.

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

## `uninstall` (and `orphans --clean`)

`data`: `plans` (array of Plan, one per app, in the order given) and `report`:

- `null` on `--dry-run`, and in every error envelope;
- otherwise an array aligned with `plans`, one entry per plan: `null` for a plan with no actions, else an ExecReport. When nothing at all is to be done, every entry is `null`.

**ExecReport**: `batch` (string, the history batch ID), `freed` (number, bytes) and `results` (array, one per planned action, same order): `{ action, ok, error, history_id }`. `action` is the planned action's `{ type, path, ... }`; `error` is string or null; `history_id` is the entry ID usable with `restore` (for a command it records the run, and there is nothing to restore).

`freed` is the sum of the `size` of every action that succeeded. As in M2 it counts bytes moved to the trash as well as bytes deleted: a trashed item still takes disk space until the trash is emptied. Read it as "removed from where it was", not as "returned to the filesystem". An action skipped because the action it `depends_on` failed has `ok` false and the `error` text `skipped: <path> was not removed`.

Exit 0 when every action succeeded, 3 when some failed.

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

Only AppImage apps can be fully uninstalled in this version. Other sources need `--leftovers-only`; otherwise exit 2 with the error envelope shown earlier. An AppImage whose file fails the safety checks (outside home, protected or excluded, or under a symlinked directory) is refused before anything is planned: exit 1 with `cannot remove <id>: <reason>; use --leftovers-only to remove only its leftovers`.

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

**History entry**: `id` (`<batch>-<index>`), `batch`, `time`, `action` (`trash`, `delete`, `edit-file`, `restore` or `command`; lines written by earlier builds may say `edit_file`, which means the same), `path`, `trashed` (null, or `{ original, files_path, info_path }`), `backup` (string or null), `size` (number), `ok` (boolean), `error` (string or null), `app_id` (string or null), `restores` (string or null), `argv` (array of string; present on `command` entries only) and `kind` (string: `uninstall`, `orphans` or `clean`, the command that made the batch; absent on `restore` entries and on lines written by earlier builds).

- `trashed` is set for `trash` entries, and on `restore` entries it repeats the trash record that was restored.
- On an `edit-file` entry, `backup` is the copy of the file as it was before the edit.
- On a `restore` entry, `restores` is the ID of the entry it restored. For a restored `edit-file`, `backup` is the **pre-restore backup**: a copy of the file as it was just before the restore overwrote it. For other entries `backup` is null.
- On a `command` entry (a `user-command` action that ran), `path` is the program's in-system path (or its bare name when it is not installed), `argv` its arguments, `size` 0, `trashed` and `backup` null. It cannot be restored.
- Failed and skipped attempts are recorded too (`ok` false, `error` set).
- A batch made by `clean` has `app_id` null and `kind` `clean` on its entries, so `history` labels it `clean`.

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
| `entry_id` | string | The restored history entry; empty string when the selector matched nothing. |
| `path` | string | The entry's original path (the selector text when nothing matched). |
| `ok` | boolean | |
| `error` | string or null | For example `already restored`, `no matching history entry`. |

A batch selector restores the batch's successful `trash` and `edit-file` entries; `delete` and `command` entries are skipped, since nothing of them can be put back (an entry ID names one anyway, and its outcome says `permanently deleted; cannot be restored` or `commands cannot be undone`). A path selector names the newest successful trash entry for that path that has not been restored yet. `--dry-run` checks and reports without changing anything. Exit 0 when every outcome is `ok`; 2 when no selector matched any history entry (every outcome has an empty `entry_id`); otherwise 3.

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

`crumbs scan [--category <name>]... [--details] [--user] [--json]` is strictly read-only: it never changes a file and never writes history. `--system` is not available yet: exit 2 with `system cleanup needs privileged support (coming in a later version)` (under `--json`, an envelope with `data: null` and a `cli` error).

`data`: `categories`, an array of **CategoryReport**, one per listed category in this fixed order: `cache`, `thumbnails`, `trash`, `temp`, `browser-cache`, `dev-cache`, `flatpak-unused`, `appimage-old`, `logs`.

- With no `--category`, all nine are listed. Categories switched off in `categories_off` (default: `dev-cache`) are listed with `skipped: true`, an empty `items` and `total` 0, and are not scanned.
- With `--category`, exactly the named categories are listed and scanned, even ones switched off in the config.

| Field | Type | Meaning |
|---|---|---|
| `category` | string | One of the nine names above. |
| `items` | array | BloatItems, largest first. |
| `total` | number | Sum of the known item sizes, in bytes. |
| `unknown_sizes` | number | Items whose size is unknown (`size` null). The real total is larger. |
| `error` | string or null | Set when this category's scanner failed. `items` is then empty. |
| `skipped` | boolean | `true` for a category that is switched off and was not named: it was not scanned. |

**BloatItem**

| Field | Type | Meaning |
|---|---|---|
| `category` | string | The category. |
| `path` | string or null | In-system path. `null` for a command item. |
| `label` | string | What to show: a name, a display path, or for a command what it does. |
| `size` | number or null | Bytes. `null` when unknown. |
| `rebuildable` | boolean | The owner recreates it on demand (caches). |
| `preselected` | boolean | Whether `clean` includes it by default. `false` for an item held back (its app is running, see `note`) and for `flatpak-unused` and `trash` items, which `clean` includes only when their category is named. |
| `note` | string or null | Why it is held back (`Firefox is running`), or what to know (`deleted permanently (temporary file)`, `decided by flatpak`). |
| `action` | string or object | `"trash"`, `"delete"`, or `{ "command": { "program", "args" } }`. |
| `scope` | string | `home`, `user-temp` or `home-trash` (see the plan action's `scope`). Always present here. |
| `extra_paths` | array of string | Further paths removed together with the item, for example the `.trashinfo` file of a trashed entry. Paths only: the safety fingerprints are internal and not shown. |
| `app_id` | string or null | The app the item belongs to, when exactly one installed app matches. |

The internal fingerprint of an item is never serialized.

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

`crumbs clean [<category>...] [-i] [--dry-run] [--yes] [--permanent] [--force] [--json] [--user]` prints the same `{ plans, report }` shape as `uninstall`, with `command` `clean`; it always has exactly one plan, with `app_id` null, or none at all when there is nothing to do. `--json` needs `--yes` or `--dry-run`, as for `uninstall`. `--system` exits 2 with the same message and error envelope as `scan`, except that `data` is `{ "plans": [], "report": null }`.

Which items are planned:

- With no category, every enabled category is scanned (all but those in `categories_off`). Naming categories scans exactly those, even ones switched off in the config.
- The items cleaned are the ones with `preselected` true. An item held back (its app is running) is left out and listed nowhere in the JSON. With `-i` you pick from all items instead.
- `flatpak-unused` and `trash` are opt-in: their items are never preselected, so they are planned only when the category is named (or ticked with `-i`). A plain `crumbs clean` never empties the trash.
- `trash` and `temp` items are always permanent deletes (`delete` actions with `scope` `home-trash` and `user-temp`, and the plan's `notes` say so). With `--permanent`, trash items of the other categories become `delete` too.
- When any action is a `trash`, the plan's `notes` include `space is freed when the trash is emptied (or use --permanent)`.

Refusals:

- If an app source or a category's scanner failed, `clean` refuses with exit 1 and `refusing: some sources failed (...); re-run with --force to continue`, unless `--force` or `--dry-run` is given (a dry run only warns). The failed sources and scanners lead `errors`, followed by the `cli` entry, and `data` is `{ "plans": [], "report": null }`.
- Nothing chosen (nothing found, or everything held back): exit 0 and `data` is `{ "plans": [], "report": [] }`, or `{ "plans": [], "report": null }` with `--dry-run`.
- Any other refusal or failure uses the error envelope described at the top, with `report` null.

Exit 0 when every action succeeded, 3 when some failed.

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
