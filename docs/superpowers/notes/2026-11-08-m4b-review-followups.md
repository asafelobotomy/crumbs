# M4b review follow-ups

M4b is done: system leftovers in `/etc` and `/opt`, path-based removal of unmanaged apps, `orphans --system`, the owned-directory index, `Scope::SystemTree` with its re-checks, stricter trash roots, script-launched app detection and the `act.rs` split. This note lists what `m4b-scope.md` asked for or inherited and M4b did not do, plus everything the task reviews parked. Each item either fails safe today or only affects what the tool reports; none is a known data-loss path. The final fix wave of M4b handles the Task 3 carries and they are not repeated here.

What M4b did take from `m4b-scope.md`: the five "in scope" items; the HomeTrash root and topdir-trash rules; bind mounts and mount namespaces before a root tree delete; `restore <batch>` on a delete-only batch; `--keep-data` leaving mime associations; `glob_match` (`**` linear, `.cache/**` matching `.cache`); and the `act.rs` split.

## Process matching

1. **pacman `claude-desktop` is not detected as running.** Its launcher is a bash script and the process basename is `claude`, so the program-path match through `/proc/<pid>/cmdline` misses it. Its caches are offered unselected only when another signal fires. Script-launched apps are otherwise covered (Task 7).
2. **argv1 data-file false positives.** A process whose argv1 is a data file under an app's directory can make that app look running. It fails safe (an extra "running" refusal).
3. **`comm` truncation** (15 bytes) can call a sibling-prefix program running. Fails safe. Carried from M4.
4. **Cross-plan dependencies** record only the first covering plan, and there is still no end-to-end CLI test of the cross-plan gate. Carried from M4.

## System leftovers and unmanaged removal (limits kept on purpose)

5. **tmpfiles.d is not re-read at execution time.** A path a tmpfiles.d file declares counts as owned when leftovers and unmanaged plans are made, but `recheck_system_tree` re-reads only the pacman database. A tmpfiles.d file added between the preview and the confirmation is not noticed. The pacman lock is also checked user-side only (see the Task 3 carries for the root-side check).
6. **Without a complete pacman database nothing in `/opt` is offered**, and unmanaged `/opt` removal is refused with `package ownership is unknown (no pacman database)`. Off Arch this stays so until M5 adds dpkg and rpm ownership.
7. **`/etc` is never listed by `orphans --system`**, and `ETC_NEVER` names also apply under `/opt`. Conservative by design.
8. **Exec basenames never prove a name** for unmanaged removal, and a leftover claimed only through an `Exec` basename is capped at Low. Apps whose files are named after something other than their name, desktop id or display name (for example `org.notify.Panel` with `notify-panel`) are leftovers-only.
9. **Entries of the same app count as non-sharing only when sure**: two desktop entries with the same program but unrelated ids are treated as sharing (fail closed).
10. **A symlink to an `/opt` program** outside the home directory, or in a protected home path, is left in place with a note and dangles after the removal.
11. **Restore of a system-tree delete** says `system files deleted as root cannot be restored`; there is nothing to restore it from. Documented, not planned.
12. **`load_pacman` cost**: about 4 s in a debug build and 2 s optimised on a 7,000-entry database, read once per batch.
13. **MCP server** is not part of M4b: planned as M4c (`docs/superpowers/specs/2026-11-08-crumbs-mcp-design.md`).

## From `m4b-scope.md`, not done

### M3 final review

14. `--keep-data` fallbacks: a launcher with a non-UTF-8 name or an unreadable one is treated as the app's own and removed with the AppImage (trashed, so restorable).

### M2 "Before M3", not verified

15. `orphans` with clean-only flags can print the uninstall-style error envelope.
16. Unsanitised edit preview lines.

### M3 "Smaller items (any time)", untouched

17. `still_stale` rebuilds the `/proc` open-file set for every temp item.
18. `/tmp` prefix matching is case-sensitive.
19. `maps` `maj:min` vs `st_dev` can mismatch on a btrfs `/tmp`.
20. Other users' file descriptors are unreadable; mtime and ctime are the backstop.
21. An existing Trash missing `files/` or `info/` makes the trash scanner fail outright, which blocks a no-argument `clean` without `--force`.
22. `**/..` in exclude patterns collapses lexically and under-shields (contrived).
23. `report[].freed` counts trashed bytes too (documented M2 semantics).
24. The failure envelope uses `report: null`; nothing-to-do uses `report: []`.
25. A cold-cache `scan` took 9 s once on the dev machine, warm runs about 4 s: profile the cache sizing.
26. Test gaps: `--permanent` for `clean`, a category named twice, the dry-run null report, a `render::report()` command-only batch, and root-skipped tests that assert nothing when run as root.

### M1 polish

27. `fmt_size(1_048_575)` prints `1024.0 KiB`.
28. Desktop `Exec` parsing recognises only a bare `env` prefix.
29. Test gaps for `--sort size` and non-ASCII column alignment.

### M4 minors

30. **Signals and timeouts:** Ctrl-C reaches only crumbs' process group, so a running user command may outlive crumbs; a daemonised descendant that keeps stdout open can stall a batch (no batch timeout); the signal tests share the process-wide `SIGQUIT` handler and can race (serialise them with a mutex).
31. **Batch behaviour:** one batch error (for example too large) fails every plan's privileged actions; output is lost if the shell itself dies; all trailing newlines of step output are trimmed; a shell killed during step 0, before any marker, is reported as a denial though that step may have partly run.
32. **`SystemPrivilegedRunner`** guards its own root, not the `Env` passed to the executor; `allow_detection_in_test` is not reentrant; a non-`PackageRemove` step that exits non-zero does not name the program in its message; runner errors map to `Refused`.
33. **Flatpak:** an app id installed in several branches or arches is removed in full by `flatpak uninstall <id>` (unverified; document it); an installation literally named `user` or `system` is ambiguous.
34. **Pacman post-check:** a half-written local database entry makes the check report "could not verify" (fails closed).
35. **Scanners:** the `journal` estimate counts active files too; `flatpak-unused` is offered whenever runtimes exist; `pkg_cache_keep = 0` deserves a note in the preview; the target-file root-owner check for `Scope::System` is not done (the directory chain already has to be root-only).
