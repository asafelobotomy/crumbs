# crumBS in uQ

What this folder adds to the [uQ](https://github.com/asafelobotomy/uq) desktop. Nothing reads
these files unless uQ is installed; the `crum-bs` and `crum-bs-bin` packages install them unconditionally. (The
`.deb` and `.rpm` install the launcher action only: uQ runs on Arch, so the Settings page would be inert there.)

| File | Installed to | What it does |
|---|---|---|
| `launcher/actions/crumbs-uninstall.appaction` | `/usr/share/uq/launcher/actions/` | "Uninstall…" in the right-click menu of every app row in Spotlight and the app menu |
| `pages/crumbs/` (`page.toml`, `Page.qml`, section components, `logic.js`) | `/usr/share/uq/settings/pages/crumbs/` | the "Storage and cleanup" page under Apps in uQ Settings |

## "Uninstall…"

uQ runs `crumbs uninstall --desktop-id <id>` in the terminal chosen in Settings → Default apps,
with the row's desktop file id (`firefox.desktop`) as one argument. crumbs:

- refuses an id that is not a plain desktop file id (`^[A-Za-z0-9._-]{1,200}\.desktop$`, not
  starting with `-` or `.`), before reading anything;
- looks up the installed app whose launchers include exactly that id (no app ids, names or
  loose matches), and refuses when no app or several apps own it;
- then behaves like `crumbs uninstall <app>`: it shows the plan and asks `Proceed?`, and a
  package that is part of the uQ desktop needs a second yes.

## "Storage and cleanup"

The page is a front end for the `crumbs` command line, as `docs/settings-pages.md` in uQ describes.
Everything it reads or changes is a `[[command]]` of `pages/crumbs/page.toml`, run with `--json`
(`docs/json-schema.md` is the contract), and `logic.js` is the only code that reads crumbs' answers.

| Section | Reads | Changes |
|---|---|---|
| Space you can free | `scan`, when the page opens and on Refresh | `clean` for categories that go to the trash, `clean-for-good` for the ones deleted for good |
| System space | `scan --system`, only when asked | `clean --system`, which asks for the password through crumbs' own prompt |
| Leftovers of removed apps | `orphans`, only when asked | `orphans --clean --select <path>` (to the trash) |
| Recent cleanups | `history`, followed through `~/.local/state/crumbs/history.jsonl` | `restore <batch>` |
| Apps and their size | `apps list --sort size`, only when asked | `uninstall <app id>` |

- Every change asks first (the `confirm` of its command), takes one name that crumbs itself printed, and runs
  alone: while one runs, every other change button waits.
- The page never forces anything: no argv or argument holds `--force`, `--desktop`, `--permanent` or `-i`, so
  crumbs' own refusals (an app that is running, a package that is part of uQ) are shown as they are. Packages
  of the uQ desktop show "Part of uQ" and have no Uninstall button.
- The category lists in `logic.js` split by what a cleanup does and equal the twenty names in
  `docs/json-schema.md`; a test fails when crumBS adds one, until the page knows it.
- AppImages that appOFholding keeps are not listed by crumbs; the page says they are managed there.

## Testing

- `node --test 'tests/uq/*.test.mjs'`: `logic.js`, `page.toml` (commands, argument patterns, confirms,
  plain words) and what the packages install. `tests/uq/fixtures/` holds crumbs' `--json` answers.
- `QML_XHR_ALLOW_FILE_READ=1 QT_QPA_PLATFORM=offscreen qmltestrunner -import tests/uq/stub -input tests/uq`:
  the page against a fake `ctx` (`tests/uq/tst_page.qml`; needs Qt 6, so it runs in CI). The stub components
  and `FakeContext.qml` are uQ's test kit as of v0.3.0-rc.1.
- `scripts/test-packaging.sh uq_files` runs each AUR `package()` in a scratch tree and checks that what it
  installs under `usr/share/uq` equals this folder; `scripts/test-packaging.sh aur` checks the built packages.
- `uqctl integrations check --no-programs uq/` (CI pins uQ v0.3.0-rc.1) checks the folder.
- `cargo test --workspace`: the lookup (`crumbs-core`, `inventory/tests.rs`) and the command
  (`crumbs-cli/tests/cli.rs`, `uninstall_by_desktop_id_*`).
- Without `--no-programs`, `uqctl integrations check uq/` also reports that `/usr/bin/crumbs` does not exist
  on a machine without crumbs installed.
