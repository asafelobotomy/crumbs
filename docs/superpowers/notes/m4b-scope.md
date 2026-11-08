# M4b scope: system leftovers and unmanaged removal

M4 (privileged removal of pacman and Flatpak apps, and the six system categories) is done. M4b takes the deletions that M4 left out because they are the riskiest: files outside the home directory that belong to no package manager. This note lists what M4b must cover and which earlier follow-ups it inherits. It is input for the M4b plan, not the plan itself.

Notation: "M1 follow-ups, Entry criteria for M4, item 2" means that section and item of `2026-11-08-m1-review-followups.md`; "M2 FU" and "M3 FU" are the M2 and M3 follow-up notes.

## In scope for M4b

1. **System leftovers.** Find `/etc/<name>` and `/opt/<name>` entries that no installed package owns and that belong to a removed or detected app, and offer them for removal with administrator rights. Spec §4 (Search locations, system level), M4 plan decision 1.
   - They are always deleted, not trashed (the trash is per user); the preview must say so.
   - Confidence is capped below High: a leftover in `/etc` can be a local edit of a live package's config.
2. **Path-based removal of unmanaged apps.** `uninstall` of an `unmanaged:` app is refused today with `removing unmanaged apps isn't supported yet; use --leftovers-only` (exit 2; `removal::MSG_UNMANAGED`). M4b removes the app's own files by path.
   - M1 follow-ups, Entry criteria for M4, item 2: `location` is a hint, not a removal target. `/opt/<first component>` can be a shared vendor directory (`/opt/google`); for a symlinked program it is the symlink. Act only when the target is unambiguous, unowned and specific to the app.
3. **The owned-directory database.** `owned_paths` drops directory entries, so a package-owned `/opt/foo/` looks unowned. M1 follow-ups, Entry criteria for M4, item 1; M2 FU, Before M4, item 5 (carried from M1). Both features above need to know which directories a package owns before treating one as unowned. The database is built from the pacman local database today (`files` sections keep the trailing `/` for directories); other package managers come with M5.
4. **A general `Scope::System`.** M4 implements the scope for one purpose: a direct child `core.*` file of `/var/lib/systemd/coredump`, with every directory from `/` down root-owned and not writable by others (`safety::check_system_chain`). M3 FU, Before M4, item 3 asked for a parent-chain check from `/` with ownership rules, and `recheck_scope` covering more than `UserTemp`. M4b needs it for `/etc` and `/opt` (new allowed roots, a per-root ownership rule, deleting a directory tree as root).
5. **Removing a directory tree as root.** M4 deletes single files (`rm -f -- <path>`). M4b removes directories, so the batch needs a tree-safe command, a re-check right before running (no symlink swapped in, the same device, nothing owned by a package since the scan), and a fingerprint that covers directories (M3 FU, Before M4, item 1, done for files in M4; `ctime` is skipped for directories).

## Not M4b (planned elsewhere)

- apt and dnf variants of `pkg-cache` and `orphan-pkgs`, dpkg, rpm and Snap sources: M5, with the distro CI matrix. The pacman-only categories report `unavailable` elsewhere (M4 plan decision 1).
- Exit 1 instead of 0 when every source failed: done in M4 for `apps list` and `doctor` (M1 follow-ups, item 5). Revisit when M5 adds sources.
- The GUI: separate spec after M4b.

## Follow-ups M4 did not do (carry into the M4b plan)

From the M2 and M3 "Before M4" lists:

- **HomeTrash roots trust `data_home`.** They are validated as private directories but not required to be under the home directory. M3 FU, Before M4. Matters more once M4b adds system roots, so that one cannot be redirected into the other.
- **Trash topdir accepted by name only.** M2 FU, Before M4, item 3. (M4 did clamp the topdir walk at the fake root, and made the history file `O_NOFOLLOW`; M4 did not change how an existing topdir trash root is accepted; re-check that before relying on it.)

From the M3 "Smaller items (any time)", none of which M4 touched:

- `still_stale` rebuilds the `/proc` open-file set for every temp item.
- `/tmp` prefix matching is case-sensitive.
- `maps` `maj:min` vs `st_dev` can mismatch on a btrfs `/tmp`.
- Other users' file descriptors are unreadable; mtime and ctime are the backstop.
- An existing Trash missing `files/` or `info/` makes the trash scanner fail outright, which blocks a no-argument `clean` without `--force`.
- `**/..` in exclude patterns collapses lexically and under-shields (contrived).
- `report[].freed` counts trashed bytes too (documented M2 semantics).
- The failure envelope uses `report: null`; nothing-to-do uses `report: []`.
- A cold-cache `scan` took 9 s once on the dev machine, warm runs about 4 s: profile the cache sizing.
- Test gaps: `--permanent` for `clean`, a category named twice, the dry-run null report, a `render::report()` command-only batch, and root-skipped tests that assert nothing when run as root.

From the M1 "Smaller polish": `fmt_size(1_048_575)` prints `1024.0 KiB`; desktop `Exec` parsing recognises only a bare `env` prefix; test gaps for `--sort size` and non-ASCII column alignment.

## Minor findings M4's own reviews deferred

Parked during the M4 task reviews (see `.superpowers/sdd/2026-11-08-crumbs-m4-privileged/progress.md`); none is a known data-loss path.

- **Signals and timeouts:** Ctrl-C reaches only crumbs' process group, so a running user command may outlive crumbs; a daemonised descendant that keeps stdout open can stall a batch (no batch timeout); the signal tests share the process-wide `SIGQUIT` handler and can race (serialise them with a mutex).
- **Batch behaviour:** one batch error fails every plan's privileged actions, and the too-large message says "clean fewer categories" even for `uninstall`; the `TooLarge` message is unclear with many core dumps; output is lost if the shell itself dies; all trailing newlines of step output are trimmed.
- **`SystemPrivilegedRunner`** guards its own root, not the `Env` passed to the executor; consider building it from `&Env`. `allow_detection_in_test` is not reentrant. A non-`PackageRemove` step that exits non-zero does not name the program in its message. Runner errors map to `Refused`.
- **Flatpak:** an app id installed in several branches or arches is removed in full by `flatpak uninstall <id>` (unverified; document it). An installation literally named `user` or `system` is ambiguous.
- **Pacman post-check:** a half-written local database entry makes the check report "could not verify" (fails closed).
- **Scanners:** the `journal` estimate counts active files too; `flatpak-unused` is offered whenever runtimes exist; `pkg_cache_keep = 0` deserves a note in the preview; target-file root-owner check for `Scope::System` is not done (the directory chain already has to be root-only).
- **Process matching:** the 15-byte `comm` truncation can call a sibling-prefix program "running" (it fails safe); only the first covering plan is recorded for a cross-plan dependency; there is no end-to-end CLI test of the cross-plan gate.
- **Structure:** `crates/crumbs-cli/src/cmd/act.rs` is about 800 lines; split it when M4b touches it again.
