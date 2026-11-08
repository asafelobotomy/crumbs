# M3 review follow-ups (carry into the next plans)

These came out of the milestone 3 per-task reviews and the whole-branch review. Every item below fails safe or is display-only today.

## Open from the final review (fix before or right after merge)

- **`restore <batch>` on a delete-only batch** (`clean temp`/`clean trash`) says `no matching history entry` and exits 2, even though `history` lists that batch. A clearer message is needed, and the case should be documented.
- **`--keep-data` fallbacks.** A launcher with a non-UTF-8 name, or one that can't be read, is treated as the app's own and removed along with the AppImage. It goes to the trash, so it can be restored.
- **`--keep-data` leaves mime associations behind** that point at the removed launcher.

## Before M4 (privilege, package removal, system paths)

- **Fingerprint:** it is `(dev, ino, is_symlink)`. Add ctime/mtime before any irreversible `PackageRemove`.
- **`SystemRunner`:** it inherits the environment and has no timeout. Privileged commands need a scrubbed environment, a timeout and their own scope check. Commands bypass `verify` today.
- **`Scope::System`:** it needs a parent-chain check from `/` with ownership rules. `recheck_scope` currently covers UserTemp only.
- **`Action::path()`** returns the program name as a fake relative path for `UserCommand`. Any new caller that verifies or prefix-matches it must special-case commands.
- **HomeTrash roots** trust `data_home` (they are private-dir validated, but not required to be under home).
- **Process matching** misses `claude`/electron-style executables, so caches of those running apps are pre-selected (they are trashed, so restorable).
- **Cross-plan dependency gate.** An AppImage stored inside another selected app's leftover folder loses its file dependency when that folder's removal fails.
- **Env test hooks.** `Env.clock_shift` and `Env.tmp_owner_uid` are `pub`. They are inert in release builds through the CLI, but a library consumer could set them. Consider gating them on `cfg(any(test, feature = "test-support"))`.

## Smaller items (any time)

- `still_stale` rebuilds the `/proc` open-file set for every temp item.
- `/tmp` prefix matching is case-sensitive.
- `maps` `maj:min` vs `st_dev` can mismatch on btrfs `/tmp`.
- Other users' fds are unreadable; mtime/ctime is the backstop.
- An existing Trash that is missing `files/` or `info/` makes the trash scanner fail outright, which blocks a no-argument `clean` without `--force`.
- `**/..` in exclude patterns collapses lexically and under-shields; it is contrived.
- `report[].freed` in JSON counts trashed bytes too (M2 semantics; documented).
- The failure envelope uses `report: null`, while nothing-to-do uses `report: []`.
- Cold-cache `scan` took 9 s once on the dev machine; warm runs take about 4 s. Profile cache sizing.
- Test gaps: `--permanent` for `clean`, a category named twice, the dry-run null report, a `render::report()` command-only batch, and root-skipped tests that assert nothing when run as root.
