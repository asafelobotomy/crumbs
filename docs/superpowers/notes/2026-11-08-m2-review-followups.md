# M2 review follow-ups (carry into M3/M4 plans)

These came out of the milestone 2 per-task reviews and the whole-branch review. M2 is user-level only and trash-first. Every item below is either fail-closed today or affects only what gets suggested. Handle each one before the milestone named.

## Before M3 (bloat scanners, `clean`)

1. **`exclude` wildcard ancestors.** `exclude = ["~/.config/*/keep"]` shields only the literal prefix (`~/.config`). It does not shield `~/.config/Foobar`, so trashing that folder takes `Foobar/keep` with it.
   - Fix: refuse any directory below which the pattern could still match.
   - Also normalise `..` in patterns.
2. **Bind mounts.** `size::crosses_device` compares `st_dev`, so a same-filesystem bind mount inside a tree is not detected before a `--permanent` delete.
   - Fix: use the mount ID (`statx` `STX_MNT_ID` or `/proc/self/mountinfo`).
3. **`restore` writes history once per batch.** The executor now appends per action. Make `restore` match, so SIGINT or one non-UTF-8 path can't lose the record.
4. **AppImage half-run at execution time.** The AppImage file is verified before the preview. If its trash action still fails during execution (TOCTOU, I/O error), the leftover actions run anyway.
   - Fix: make an app's leftover actions depend on its file action succeeding.
5. **Autostart entries are matched only by desktop ID or Exec.** `~/.config/autostart/gutcheck.desktop`, written by appofholding, is a Low orphan while GutCheck is installed.
   - Fix: also name-match autostart entries.
6. **Small items:**
   - `orphans` with clean-only flags prints the uninstall-style error envelope.
   - `Context::load` errors have no JSON envelope.
   - `doctor` discovers with `Config::default()`.
   - `apps list --source` doesn't scope errors.
   - `glob_match` with several `**` is exponential, and `.cache/**` doesn't match `.cache`.
   - `restore <path>` picks the newest entry even when it was already restored.
   - Dotfiles are labelled `dot-dir`.
   - `--keep-data` leaves AppImage launchers dangling.
   - Edit preview lines aren't sanitised for control bytes.

## Before M4 (privilege, package removal, system paths)

1. **Allowed roots per finding.** `safety::verify` hard-codes "strictly under home". Spec §8 wants allowed roots per finding.
   - Add a scope parameter rather than a second gate.
2. **Inode reuse.** `Fingerprint` is `(dev, ino, is_symlink)`. Add ctime or mtime before any irreversible action (`PackageRemove`).
3. **Trash topdir and history hardening:**
   - A topdir trash root is accepted by name only.
   - The history file is opened without `O_NOFOLLOW`, and existing modes aren't tightened.
   - The topdir walk isn't clamped at `env.root` in fake roots.
4. **Process-match noise.** `running_processes` misses `(deleted)` exe links and matches generic exec names (`python3`, `electron`). Today this only causes extra confirmations.
5. **Carried from M1 follow-ups:** owned directories, unmanaged `location` as a hint only, custom Flatpak installations, `SystemRunner` environment, and exit 1 when every source fails.

## Accepted behaviour (decided during M2)

- **Orphans are never High.** "Not detected" is not "not installed": on the dev machine, Brave (via `brave-origin-bin`) and Cursor were live.
  - A curated rule naming an app that isn't detected gives Medium.
  - Anything modified in the last 30 days is capped at Low.
- **Flatpak apps:** matches outside `~/.var/app/<id>` (and their own launcher, icons and mime entries) are capped at Medium.
- **`--leftovers-only` keeps the app working.** It keeps the app's launchers (by ID or by Exec), its icons and its mime associations.
- **Trash dir permissions:** existing trash dirs must be real directories, owned by the user, and not group- or other-writable. The usual 0755 is fine.
