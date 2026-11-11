# crumBS in uQ

What this folder adds to the [uQ](https://github.com/asafelobotomy/uq) desktop. Nothing reads
these files unless uQ is installed; the `crum-bs` package installs them unconditionally.

| File | Installed to | What it does |
|---|---|---|
| `launcher/actions/crumbs-uninstall.appaction` | `/usr/share/uq/launcher/actions/` | "Uninstall…" in the right-click menu of every app row in Spotlight and the app menu |

## "Uninstall…"

uQ runs `crumbs uninstall --desktop-id <id>` in the terminal chosen in Settings → Default apps,
with the row's desktop file id (`firefox.desktop`) as one argument. crumbs:

- refuses an id that is not a plain desktop file id (`^[A-Za-z0-9._-]{1,200}\.desktop$`, not
  starting with `-` or `.`), before reading anything;
- looks up the installed app whose launchers include exactly that id (no app ids, names or
  loose matches), and refuses when no app or several apps own it;
- then behaves like `crumbs uninstall <app>`: it shows the plan and asks `Proceed?`, and a
  package that is part of the uQ desktop needs a second yes.

## Testing

- `cargo test --workspace`: the lookup (`crumbs-core`, `inventory/tests.rs`) and the command
  (`crumbs-cli/tests/cli.rs`, `uninstall_by_desktop_id_*`).
- `uqctl integrations check uq/` checks the file (on a machine without crumbs installed it also
  reports that `/usr/bin/crumbs` does not exist).
