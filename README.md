# crumBS

crumBS is a Linux command-line tool that lists the apps installed on your system across pacman, Flatpak, AppImage and unmanaged `.desktop` sources, finds the files they leave behind, finds space you can reclaim (caches, the trash, old temporary files, logs, superseded AppImages), and moves the items you pick to the trash. It is pre-alpha software.

**Status:** M4c: `crumbs mcp` serves crumbs to AI agents, with the user's confirmation for every change. M4b: `uninstall` fully removes pacman and Flatpak apps (and AppImages), and now also unmanaged apps by path; `uninstall` and `orphans --system` find system leftovers in `/etc` and `/opt`. `scan` and `clean` handle system bloat with `--system`, on top of M3's reclaimable space in your own files and M2's leftovers, orphans, history and restore. Everything that needs administrator rights runs in one batch behind one prompt. M5: Debian/Ubuntu (dpkg, apt), Fedora (rpm, dnf) and Snap work like Arch, checked in containers by `scripts/distro-check.sh`. M6: releases ship as an AppImage, `.deb`, `.rpm`, AUR packages and a static binary (see "Install").

## Install

Every release on [GitHub](https://github.com/asafelobotomy/crumbs/releases) has the same static x86_64 binary in several wrappings, and a `SHA256SUMS` file to check them with (`sha256sum -c SHA256SUMS --ignore-missing`). crumbs is a command-line tool: run it from a terminal.

- **AppImage** (any distro): download `crumbs-<version>-x86_64.AppImage`, then `chmod +x` it and run it. It needs no `libfuse2`; where FUSE is not available at all (some containers), run it with `--appimage-extract-and-run` or `APPIMAGE_EXTRACT_AND_RUN=1`. Kept in `~/Applications`, `~/.local/bin` or `~/Downloads`, crumbs lists its own AppImage like any other, so `crumbs uninstall` can remove it.
- **Debian and Ubuntu:** `sudo apt install ./crumbs_<version>_amd64.deb`.
- **Fedora:** `sudo dnf install ./crumbs-<version>-1.x86_64.rpm`.
- **Arch and Manjaro:** once published to the AUR, `yay -S crumbs` builds from source and `yay -S crumbs-bin` installs the release binary.
- **Static binary:** `crumbs-<version>-x86_64-linux-musl` has no dependencies; put it on your `PATH` as `crumbs`.

The packages install `/usr/bin/crumbs`, so crumbs lists itself as an app of your package manager (`dpkg:crumbs`, `rpm:crumbs`, `pacman:crumbs` or `pacman:crumbs-bin`) and can uninstall itself.

### From source

```sh
cargo build --release
./target/release/crumbs --help
```

`crumbs mcp` (see "Using crumbs from an AI agent") is built in by default. `cargo build --no-default-features` builds the CLI without it, and without the `rmcp`, `tokio` and `schemars` dependencies; the `mcp` subcommand then does not exist.

## Commands

- `crumbs apps list [--source <source>] [--all] [--sort name|size] [--json]` lists installed apps.
- `crumbs apps show <app> [--show-low] [--json]` shows one app and its leftovers, system leftovers in `/etc` and `/opt` included (marked `root`).
- `crumbs uninstall <app>... [--leftovers-only] [--keep-data] [--show-low] [-i] [--dry-run] [--yes] [--permanent] [--force] [--json]` removes an app and its leftovers. pacman apps are removed with `pacman -Rn` (the exact package list its simulation printed, dependencies included; the preview shows `also removed: ...`), Debian/Ubuntu packages with `dpkg --purge <exact list>` after an `apt-get -s purge` simulation (a removal of an essential package is refused), Fedora packages with `rpm -e <exact list>` after a `dnf remove --assumeno` simulation (dnf5 and dnf4; protected packages are refused). dpkg and rpm never add packages of their own: if something new depends on a listed package by the time the removal runs, they refuse instead of removing more than the preview showed; snaps with `snap remove --purge`, Flatpak apps with `flatpak uninstall` for their installation (user, system or a named one), AppImages by trashing the file. An unmanaged app (`unmanaged:<id>`) has its own files removed by path when that is unambiguous, and is otherwise refused with a reason (see "System leftovers and unmanaged apps"); `--leftovers-only` is the fallback. With `--leftovers-only` the app stays, and so do its launchers, icons and file-type associations. `--keep-data` keeps the app's settings and data and removes only its caches; when the AppImage file itself goes, its own launchers and icons go with it, so nothing is left pointing at a removed file. If the app is running, crumbs asks `<app> is running; continue anyway?` (without a terminal it refuses with exit 1; `--force` goes on). After `Proceed?` it checks running apps again: an app that started while you were answering makes the run refuse with exit 1, before anything changes.
- `crumbs orphans [--show-low] [--json]` lists per-user files that belong to no installed app.
- `crumbs orphans --system [--show-low] [--json]` lists unowned directories directly under `/opt`; it never lists `/etc`.
- `crumbs orphans --clean (--select <path>... | -i) [--dry-run] [--yes] ...` removes chosen orphans; with `--system` they are deleted as root. Nothing is removed unless you name it.
- `crumbs history [--limit N] [--json]` lists past removals, newest first, each batch labelled with the app it uninstalled, `orphans`, `clean` or `restore`.
- `crumbs restore <batch | entry | path>... [--dry-run] [--json]` puts items back. Restoring a batch skips what was permanently deleted, run as a command or removed as a package; naming such an entry by its ID says why it cannot come back.
- `crumbs scan [--category <name>]... [--details] [--user | --system] [--json]` lists reclaimable space by category. `--system` lists the system categories instead. It only reads: it changes nothing, needs no administrator rights and writes no history. With `--details` every item is listed. Naming categories scans exactly those, even ones switched off in the config.
- `crumbs clean [<category>...] [-i] [--dry-run] [--yes] [--permanent] [--force] [--user | --system] [--json]` frees the space `scan` found. With no category it cleans every enabled user category; `--system` cleans the system categories instead. Naming categories cleans exactly those, and naming a system category implies `--system`. `-i` opens a checklist over all items. `--user` with `--system`, or a category of the other kind, is a usage error (exit 2).
- `crumbs doctor [--json]` reports which sources and tools crumbs can use.
- `crumbs mcp` serves crumbs to an AI agent over the Model Context Protocol on stdin and stdout (see "Using crumbs from an AI agent").

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

### System leftovers and unmanaged apps

**System leftovers.** When you uninstall an app, crumbs also looks for directories and files directly under `/etc/<name>` and `/opt/<name>` that no installed package owns and that belong to that app. They are the riskiest thing crumbs can find, so:

- they are **never selected by default**: a plain `uninstall` lists a count (`note: 1 system leftover not selected (pick with -i)`) and removes nothing there; you pick them with `-i`, where they are marked `(root)`;
- they are **deleted permanently, as root**, not moved to the trash, and `crumbs restore` cannot bring them back (`system files deleted as root cannot be restored`);
- their confidence is never above medium, because something in `/etc` can be a local edit of a package you still have;
- one you pick goes even if removing the app itself fails: you chose it, and no package owns it;
- a runtime directory a package declares in tmpfiles.d (`/usr/lib`, `/usr/local/lib`, `/run` or `/etc/tmpfiles.d`) is never one, and a declared pattern such as `/opt/foo*` protects exactly the entries it could match (`/opt/foobar`, not `/opt/bar`);
- when the size of one cannot be read, the preview shows `-` and the total says `+ unknown`.

**Unmanaged apps.** `crumbs uninstall unmanaged:<id>` removes the files of an app that no package manager installed: the `/opt/<dir>` its program sits in (as root), or its directory in `~/opt` or `~/Applications`, or the single program in `~/bin` or `~/.local/bin` (trashed). It acts only when the target is unambiguous, and otherwise refuses with a reason and `--leftovers-only` as the fallback (exit 2):

- `its program is in a protected or shared location`, for example `/usr/local`;
- `<path> belongs to a package`: a package owns something at or below it, or a tmpfiles.d file declares it;
- `package ownership is unknown (no pacman database)` or `(tmpfiles.d unreadable)`: crumbs will not guess that an `/opt` directory is unowned;
- `<path> is shared with <app>`: another detected app lives there;
- `<path> does not match the app's name`: a vendor directory such as `/opt/google` is not named after the app. Only the app's own names count, never a name taken from its `Exec` line.

Example (a throwaway test tree):

```
$ crumbs uninstall unmanaged:inky.desktop --dry-run
Inky  unmanaged:inky.desktop
ACTION            SIZE     PATH       REASON
delete (as root)  4.0 KiB  /opt/inky  the app's own files
    - as root: '/usr/bin/rm' '-rf' '--one-file-system' '--' '/opt/inky'
note: system files are deleted permanently with administrator rights and cannot be restored
note: these steps run with administrator rights; you will be asked once

will permanently delete 1 item (4.0 KiB)
note: 1 system leftover not selected (pick with -i)

$ crumbs uninstall unmanaged:chromething.desktop --dry-run
error: removing this unmanaged app's files isn't possible: /opt/google does not match the app's name; use --leftovers-only
```

**`orphans --system`** lists the directories directly under `/opt` that no package owns and no detected app claims (`/etc` is never listed: an orphan there cannot be told from a local edit). It only reads. To delete, name them: `crumbs orphans --system --clean --select /opt/old-tool`, or use `-i`.

```
$ crumbs orphans --system
CONF  SIZE     KIND         PATH           REASON
low   4.0 KiB  system-data  /opt/old-tool  not owned by any package and matched by no app

1 orphans, 4.0 KiB
```

### Administrator rights

Only these need them:

- removing a pacman, apt or dnf package, a snap, a system Flatpak app or a Flatpak app in a named installation;
- the system categories (`clean --system`);
- deleting a system file (a core dump) or a system leftover or unmanaged app under `/etc` or `/opt`.

Removing a per-user Flatpak app, an AppImage, leftovers and every user category need none. A run that has such steps asks **once**: crumbs joins them into one script and runs it through `run0` (on systemd 256 or newer), else `pkexec`, else `sudo` (`sudo` only when it can ask on a terminal). When crumbs already runs as root, the script runs with the system `sh` directly and nothing asks; per-user temp cleanup is then not offered (`unavailable (not available as root)`). The preview says `these steps run with administrator rights; you will be asked once`, and shows each command exactly as it will run. `--dry-run`, `scan` and `apps` never ask.

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

With `--json` they are in `data.manual_commands`. Leftovers that depend on a removal that did not happen are left alone too, and never listed. When the list holds an `rm -rf --one-file-system` (a system leftover or unmanaged app), a last line says `check that nothing is mounted inside these paths before running the rm commands`: run by hand, crumbs' own mount checks do not happen.

If the root shell stops part-way (it was killed after some steps had finished), that is not exit 5: the steps that finished are reported as usual, the step it was in fails with `the privileged run stopped during this step; it may have partly run`, the later ones with `not run: the privileged run stopped`, and crumbs exits 3 with `error: the privileged run stopped before it finished; ...`. The history records what ran.

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

A removed package is recorded in `history`, but crumbs cannot restore packages; `crumbs restore <entry id>` prints the commands that reinstall what was removed (`pacman -S <app>`, and `pacman -S --asdeps <deps>` for the packages removed with it), also when the removal failed part-way. The leftovers go to the trash as usual and `crumbs restore` brings them back.

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

Exit codes: 0 ok, 1 error, 2 usage error or ambiguous app, 3 partial failure (including a privileged run that stopped part-way), 4 aborted by the user, 5 administrator rights needed but unavailable or denied.

## Using crumbs from an AI agent

`crumbs mcp` is an [MCP](https://modelcontextprotocol.io) server over stdio. An MCP client such as Claude Code or Claude Desktop starts it, and the agent can then look at your machine and prepare clean-ups with the same data the CLI shows. **Nothing changes until you say yes, in the client, to the exact plan.**

### What an agent can and cannot do

It has 11 tools:

- **Read:** `doctor`, `apps_list`, `app_show`, `scan`, `orphans`, `history`. They change nothing.
- **Preview:** `preview_uninstall`, `preview_clean`, `preview_orphans`, `preview_restore`. Each builds the plan the matching CLI command would run, changes nothing, and returns the plan, a `plan_id` and the preview text the CLI would print.
- **Apply:** `apply` takes only a `plan_id`.

The limits that hold whatever the agent does:

- **Every change needs your yes.** `apply` asks you through the client, showing the whole preview followed by `Apply this plan?`. Only your answer to that question counts. No tool argument can confirm a plan. Declining, cancelling or letting the question time out runs nothing.
- **Agents cannot add anything crumbs did not find.** `select` (in `preview_uninstall` and `preview_clean`) and `paths` (in `preview_orphans`) accept only items that crumbs itself finds for that same request, and a path it did not find is refused. System leftovers in `/etc` and `/opt` are planned only when named. Without `select` the CLI's defaults apply.
- **Plans are single-use and expire after 10 minutes.** A plan is removed when it runs, when you say no, and when it expires; a yes that arrives after the plan expired runs nothing. A server restart drops them all. At most 64 plans can be pending, and a preview past that is refused until some are applied or expire.
- **All the CLI's safety checks still run.** A plan keeps the fingerprints from its scan, every path is checked again right before it is acted on, protected paths are refused, and the history is written, so `crumbs restore` works as usual.
- **A running app is never closed or forced.** If an app in the plan is running, `apply` refuses and tells the agent to preview again once it is closed; the CLI's `--force` does not exist here.
- **Administrator rights go through polkit, never `sudo`.** Root steps run in one batch through `run0` or `pkexec`, with stdin from `/dev/null`, so the usual polkit prompt appears on your desktop. `sudo` needs a terminal and is never used.

### Confirmation

The confirmation is a protocol feature, so it reaches you directly and not through the agent:

- A client that starts with `initialize` gets `elicitation/create`: the server sends a form whose only field is `confirm`. This holds even when the client asks for protocol 2026-07-28 in `initialize`, because the server then settles on 2025-11-25.
- A client on the 2026-07-28 inline lifecycle (`server/discover`, then `_meta` with `protocolVersion` and `clientCapabilities` on every request) gets an `InputRequiredResult` (multi round trip) carrying the same form, and retries `apply` with your answer.
- On that lifecycle a request without `_meta` is rejected by the MCP library (error -32602) and runs nothing. Any other mismatch, such as an answer for another plan or under another key, is refused too, and nothing runs.

A client that cannot show forms (it did not declare elicitation with form mode) gets an error that names the command to run in a terminal instead, for example `this client can't ask you to confirm; run this in a terminal instead: crumbs uninstall inky --leftovers-only`. Nothing runs, and the plan stays. Where a selection has no CLI flag the command ends with `-i`, and the message says `pick the same items with -i`.

### Results of `apply`

- `{ "applied": false, "reason": "declined" }`, or `"cancelled"` or `"timed out"`: you said no (or did not answer), nothing ran.
- `{ "applied": true, "report": [...], "exit_code": 0 }`: the plan ran. The report is the CLI's `--json` report. A partial failure (exit 3) has the same shape with `exit_code` 3 and the tool error flag set, and `applied` false when no action succeeded. A restore has `outcomes` instead of `report`.
- **Exit 5, administrator rights needed but not available or not granted.** The result is not an error. It holds `exit_code` 5, an `error` text, and `manual_commands`: the commands to run yourself, one per privileged step, exactly as the CLI prints them. When a command is an `rm -rf --one-file-system`, `note` says to check that nothing is mounted inside those paths first, because run by hand crumbs' own checks do not happen. When the prompt was denied, the steps that did not need rights still ran: then `applied` is true and `report` holds what ran. A plan whose every step needed rights gives `applied` false and no `report`. `escalation_output` holds what the escalation program printed, when it printed anything. With no escalation program at all, nothing runs and `applied` is false.
- **Refusals** (an unknown or used plan, a running app, a changed file) are tool errors whose text is the CLI's message and whose structured content is `{ "exit_code", "details" }`.

`applied` is `true` when at least one action succeeded (for a restore, when at least one outcome is `ok`), and `false` otherwise: you said no, there was no escalation program, the prompt was denied for a plan made only of root steps, or every action failed.

### Things to know

- **A graphical polkit agent must be running** for root steps. Without one (a bare TTY or SSH session, for instance) the escalation fails and `apply` returns the exit-5 result above, with the commands to run yourself.
- **One `apply` at a time.** On the `elicitation/create` path, while one `apply` waits for your answer, other `apply` calls wait until it is answered (or times out) and has run. On the `InputRequiredResult` path the server keeps nothing waiting between the rounds.
- **Running apps are checked again when the plan runs**, after your answer. The CLI does the same after `Proceed?`: an app that starts while you answer makes the run refuse (exit 1), and nothing changes.
- **Keep stdin open.** The server runs until the client closes stdin, and after that the MCP library waits only 5 seconds for responses still in flight, so a client that closes stdin right after sending `apply` can lose the result.
- **Startup may tighten the history files.** Like the CLI, the server makes a looser mode on `history.jsonl` and its directory owner-only. The server sends this and other warnings (config and rule problems) as MCP log messages at level `warning`; stdout carries only the protocol and stderr stays quiet.
- **Tool arguments** mirror the CLI's flags. Together, `user` and `system` are an error for `scan`, `orphans` and `preview_clean`, as in the CLI. [docs/json-schema.md](docs/json-schema.md#mcp-tools-crumbs-mcp) lists every tool's arguments and results.

### Client configuration

Claude Code:

```sh
claude mcp add crumbs -- crumbs mcp
```

Claude Desktop, in `claude_desktop_config.json`:

```json
{
  "mcpServers": {
    "crumbs": { "command": "crumbs", "args": ["mcp"] }
  }
}
```

`crumbs` must be on the client's `PATH`; otherwise give the full path as `command`. Without the `mcp` feature (`--no-default-features`) there is no `crumbs mcp`.

## Supported systems

| | Apps | Package removal | `pkg-cache` / `orphan-pkgs` |
|---|---|---|---|
| Arch, Manjaro | pacman (`/var/lib/pacman/local`) | `pacman -Rn` | `paccache` (or `pacman -Sc`) / `pacman -Qdtq` closure |
| Debian, Ubuntu | dpkg (`/var/lib/dpkg/status`) | `dpkg --purge` (simulated with `apt-get -s purge`) | `apt-get clean` / `apt-get -s autoremove --purge` list, purged with `dpkg --purge` |
| Fedora | rpm (`rpm -qa`) | `rpm -e` (simulated with `dnf remove --assumeno`) | `dnf clean packages` / `dnf autoremove --assumeno` list, removed with `rpm -e` |
| any | Flatpak, Snap, AppImage, unmanaged | `flatpak uninstall`, `snap remove --purge`, trash, by path | — |

System leftovers and unmanaged-app removal under `/opt` use every package database present (pacman, dpkg and rpm together); a database counts only when it has content (a non-empty dpkg status with an info directory, an rpm database directory), so dpkg or rpm-tools installed as tools on Arch change nothing. Root deletes of `/etc` and `/opt` entries wait while any package manager is running. zypper is not supported yet.

`scripts/distro-check.sh` builds and tests crumbs in rootless podman containers (`archlinux`, `debian:trixie`, `ubuntu`, `fedora`), runs read-only smoke tests and a real `crumbs uninstall sl` inside each container; the CI workflow runs the same script per image.

## Safety

- **Trash first.** Files go to the freedesktop trash, not the void. Only `--permanent` deletes, and it says so in the preview. The exceptions are emptying the trash and clearing `/tmp` (`clean trash`, `clean temp`), which are always permanent and say so, as are the system categories, package removals and system leftovers (they run commands as root).
- **Careful with `/tmp`.** `temp` only runs when `/tmp` is owned by root and has the sticky bit. Only your own files and folders directly in `/tmp` are considered, only when older than `temp_age_days`, and never sockets, pipes, devices, symlinks, names that look like live sessions (X, tmux, ssh, dbus, ...) or anything a running process has open. Each is checked again right before it is deleted, and `temp` refuses to run as root.
- **Privileged steps are simulated, verified and quoted.** A pacman removal is first simulated as you with `pacman -Rs --print`; if pacman refuses (a dependency would break), crumbs refuses too. The real command then names exactly the packages the simulation showed (`-Rn --noconfirm <names>`, never `-s`), so it cannot remove more than the preview listed. A pacman database lock (`db.lck`) stops the run (`pacman is busy`) before anything is asked. Afterwards crumbs reads the pacman database again, and a package still installed is reported as a failure, whatever the command said. Every program runs by its absolute path from the system directories with a fixed `PATH` and `LD_PRELOAD`/`LD_LIBRARY_PATH` cleared, every argument is single-quoted, names are checked to be plain package or app names, and a system file is checked again right before it is deleted as root. Flatpak is always told which installation (`--user`, `--system` or `--installation=<name>`) and never `--delete-data`: an app's data in `~/.var/app/<id>` is trashed as a restorable leftover instead. In tests and fake roots crumbs never escalates at all.
- **System leftovers are deleted with care.** Only a direct child of `/etc` or `/opt` is ever a target, and every directory from `/` down to it must be owned by root and not writable by others, so no user can swap a path under crumbs. Right before the root step crumbs checks again: the target is unchanged since the preview, meaning it is still the same directory or file (the same inode), not that its contents are unchanged; **nothing is mounted at or below it** (it refuses otherwise, and also when the mount table cannot be read); no package owns anything in it now (the pacman database and tmpfiles.d are read again); and pacman holds no lock. The pacman lock is checked before the prompt and again by the root step itself, which refuses if `db.lck` appeared in between. The root step also compares its **mount namespace** with yours (with the system `readlink`, by its absolute path) and refuses to delete if they differ, so the mount check describes the tree root sees, then runs `rm -rf --one-file-system`, which never crosses into another filesystem.
- **The history is private.** `history.jsonl` is opened without following symlinks; a symlinked directory or file is refused, and a looser mode is made owner-only (group and other bits cleared) with a warning; the owner's read and write (read, write and search for the directory) are restored when missing.
- **Preview before anything happens.** `uninstall`, `orphans --clean` and `clean` show every path with its size and the reason it was chosen, then ask. `--dry-run` shows the plan and stops.
- **No surprises in scripts.** Without a terminal, `--yes` is required (or `--dry-run`); `--json` needs one of the two as well.
- **Protected paths.** crumbs refuses system locations, your home directory itself and other paths that should never be removed, re-checks every path right before acting, and skips anything that changed since the scan. Two keys in `~/.config/crumbs/config.toml` add your own (paths start with `/` or `~/`):
  - `protect = ["~/.config/keepme"]`: that path, everything under it and every folder that contains it are never found or removed.
  - `exclude = ["~/.config/Foo/keep", "~/.cache/*-tmp"]`: glob patterns (`*` matches within one path segment, `**` across segments). A path that matches, everything under it, and every folder that contains the pattern's fixed part (the text up to the last `/` before the first `*`, or the whole pattern without wildcards) are never found or removed. So excluding `~/.config/Foo/keep` keeps `~/.config/Foo` as well. `scan` and `clean` obey both keys too: an excluded item is never listed, and neither is a folder that a pattern could match something below.
- **Everything is logged and reversible.** Each removal is recorded in `history`; `crumbs restore` puts trashed items back, and files crumbs edited come back from a backup kept next to them.

## Docs

- [JSON output schema](docs/json-schema.md), with the `crumbs mcp` tools
- [MCP server design](docs/superpowers/specs/2026-11-08-crumbs-mcp-design.md)
- [Design](docs/superpowers/specs/2026-11-08-crumbs-design.md)

## Releasing

Every artifact comes from one script, which also runs in CI on a tag push:

1. Set the version in `Cargo.toml` (`[workspace.package]`) and add a `## <version>` section to `CHANGELOG.md`, then commit.
2. Run `scripts/release.sh <version>` on a clean tree. It builds into `dist/`:
   - the static binary;
   - the AppImage;
   - the `.deb` and `.rpm`;
   - the source tarball `crumbs-<version>.tar.gz` (what the AUR `crumbs` package builds from);
   - `LICENSE`, `SHA256SUMS` and `RELEASE_NOTES.md`;
   - `dist/aur/{crumbs,crumbs-bin}` with real checksums.

   It needs `appimagetool` (or `APPIMAGETOOL`), `rpmbuild`, `rustup`, `python3` and `git`. It installs the pinned `cargo-deb` into `~/.cache/crumbs-tools` when it is missing. It refuses to run unless the version in `Cargo.toml`, the AUR `PKGBUILD`s and `.SRCINFO`s and the AppStream metainfo all match.
3. Check the artifacts with `scripts/test-packaging.sh`, which needs podman. It runs each artifact in containers: the binary on four distros, the AppImage without FUSE, install and removal of the `.deb` and `.rpm`, and an AUR build.
4. Tag and push: `git tag v<version> && git push origin v<version>`. `.github/workflows/release.yml` then runs `scripts/release.sh` and creates the GitHub release with every artifact.
5. Publish to the AUR: download the workflow's `aur` artifact, or use your local `dist/aur/`, and push each folder to its AUR repository (`ssh://aur@aur.archlinux.org/crumbs.git`, `.../crumbs-bin.git`). Their checksums are of the files the release uploads.

`scripts/check-licenses.sh` lists any dependency whose licence is not GPL-compatible permissive.

## Licence

crumBS is free software under the GNU General Public License, version 3 or (at your option) any later version. See [LICENSE](LICENSE).
