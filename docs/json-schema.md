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
| `command` | string | `apps list`, `apps show`, `doctor`, `orphans`, `uninstall`, `history` or `restore`. |
| `data` | object | The command's payload, described per command below. |
| `errors` | array | Non-fatal problems and refusals. Entries are `{ "source": string, "message": string }`. |

`errors[].source` is one of:

- a source name: `pacman`, `flatpak`, `appimage`, `unmanaged` (that source failed; the others still ran);
- `config` (a `config.toml` entry was dropped; the config file is never fatal because of this);
- `rules` (a leftover rules file could not be used);
- `history` (`history` only: unreadable lines of `history.jsonl` were skipped);
- `cli` (the command refused or failed; see below).

### Error envelope

When `uninstall` or `orphans --clean` is run with `--json` and refuses or fails (bad flags, unknown or inexact app, source errors without `--force`, a running app, a user "no", ...), it still prints a complete envelope on stdout, with an empty plan, a null report, and one `cli` entry whose `message` is the same text printed to stderr after `error: `:

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

The exit code tells you which kind of failure it was (see below). `apps show` reports "no match" and "ambiguous" through its own `data` shape instead (below). Unexpected internal failures (for example an unreadable state directory) exit 1 with a message on stderr only, and no JSON.

## Exit codes

| Code | Meaning |
|---|---|
| 0 | Success (including a dry run and "nothing to do"). |
| 1 | Error, or `uninstall`/`orphans --clean` refused because a source failed (override with `--force`) or an app is running, or `uninstall` refused an AppImage whose file fails the safety checks. |
| 2 | Usage error: bad flags, ambiguous or not-found app, `--json` without `--yes` or `--dry-run`, a destructive command run non-interactively without `--yes`, or removing a non-AppImage app without `--leftovers-only`. |
| 3 | Partial failure: some source failed while another worked, or some actions/restores failed. |
| 4 | Aborted by the user at a prompt. |
| 5 | Privilege escalation unavailable (reserved; not returned by M2 commands). |

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
| `kind` | string | `config`, `cache`, `data`, `state`, `dot-dir`, `flatpak-data`, `snap-data`, `autostart`, `desktop-entry`, `icon`, `systemd-unit` or `mime-assoc`. |
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

Each action is `{ "action", "size", "reason" }`. `action` is tagged by `type`: `trash` and `delete` carry `path`; `edit-file` carries `path` and `remove_lines` (array of string; the file is edited and a backup is kept next to it). `size` is bytes (0 for `edit-file`).

## `apps list`

`data`: `apps` (array of App), `statuses` (array of `{ source, available, app_count, skipped_reason }`; `skipped_reason` is string or null). `errors` carries source failures and config notes. Exit 3 on partial failure.

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

`data`: `crumbs_version` (string), `distro` (`{ id, id_like, name }`), `sources` (array of `{ source, available, app_count, skipped_reason }`), `escalation` (null, or `{ kind, program, requires_tty }` with `kind` one of `run0`, `pkexec`, `sudo`), `trash` (`{ path, exists, writable }`), `data_dirs` (array of string) and `errors` (array of `{ source, message }`, the same list as the envelope's `errors`). Exit 3 on partial failure.

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

**ExecReport**: `batch` (string, the history batch ID), `freed` (number, bytes) and `results` (array, one per planned action, same order): `{ action, ok, error, history_id }`. `action` is the planned action's `{ type, path, ... }`; `error` is string or null; `history_id` is the entry ID usable with `restore`.

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
| `label` | string | The app ID, `orphans`, or `restore` for a batch made by `restore`. |
| `items` | number | Number of entries. |
| `size` | number | Bytes of the entries that succeeded. |
| `restored` | number | Entries of this batch that a later successful restore put back. |
| `entries` | array | History entries (below). |

**History entry**: `id` (`<batch>-<index>`), `batch`, `time`, `action` (`trash`, `delete`, `edit_file` or `restore`), `path`, `trashed` (null, or `{ original, files_path, info_path }`), `backup` (string or null), `size` (number), `ok` (boolean), `error` (string or null), `app_id` (string or null) and `restores` (string or null).

- `trashed` is set for `trash` entries, and on `restore` entries it repeats the trash record that was restored.
- On an `edit_file` entry, `backup` is the copy of the file as it was before the edit.
- On a `restore` entry, `restores` is the ID of the entry it restored. For a restored `edit_file`, `backup` is the **pre-restore backup**: a copy of the file as it was just before the restore overwrote it. For other entries `backup` is null.
- Failed attempts are recorded too (`ok` false, `error` set).

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

`--dry-run` checks and reports without changing anything. Exit 0 when every outcome is `ok`, otherwise 3.

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
