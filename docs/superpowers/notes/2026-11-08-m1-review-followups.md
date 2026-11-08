# M1 review follow-ups (carry into M2/M4 plans)

Recorded from the milestone 1 per-task and whole-branch reviews. M1 is read-only, so none of
these can delete data today. Each item below must be handled by the milestone named before that
milestone ships destructive behaviour.

## Entry criteria for M2 (leftovers, trash, uninstall)

1. **Unique app IDs.** `appimage:<file name>` collides when the same file exists in two scan dirs
   (already true on the dev machine: `Grok_Bot_0.79.2.AppImage` in `~/Applications` and
   `~/Downloads`). `flatpak:<id>` collides for the same app installed per-user and system-wide.
   Include the directory / installation in the ID before `docs/json-schema.md` freezes schema v1.
2. **`docs/json-schema.md`** (spec §9) does not exist yet. Write it.
3. **Exact resolution for destructive commands.** `find_app` rule 6 (`normalize_name`) is looser
   than the spec's resolvers. `uninstall` must require an exact match or confirm the resolved app.
4. **Partial inventories.** `find_app` can resolve uniquely only because a source failed; `apps
   show` exits 0 while `apps list` exits 3 with the same errors; `--source` doesn't scope errors.
   Destructive commands must refuse or warn when any source errored.
5. **`CRUMBS_ROOT` / `--root` ships in the production binary**, and `Env::fake`'s guard trusts
   `TMPDIR` (with `TMPDIR=/` any directory is accepted). Gate the flag (feature or
   `debug_assertions`) or refuse destructive/privileged actions when `root != "/"`.
6. **AppImage detection gaps:** Type-1 (`AI\x01`) magic is not recognised, and an unreadable or
   non-canonicalisable program falls through toward "unmanaged" instead of being skipped.
7. **EPIPE:** `crumbs apps list | head` prints `error: Broken pipe` and exits 1.

## Entry criteria for M4 (privilege, system cleanup, path-based removal)

1. **Owned directories.** `owned_paths` drops directory entries, so a package-owned `/opt/foo/`
   looks unowned. Spec §4 system leftovers and unmanaged `/opt` removal need directory ownership.
2. **Unmanaged `location` is a hint, not a removal target.** `/opt/<first component>` can be a
   shared vendor dir (`/opt/google`); for a symlinked program it is the symlink. Verify the target
   is unambiguous, unowned and app-specific before acting.
3. **Custom Flatpak installations** are mapped to System with `/var/lib/flatpak` paths; `flatpak
   uninstall --system` would target the wrong installation.
4. **`SystemRunner` environment.** It inherits `LD_PRELOAD`, `LD_LIBRARY_PATH` and `PATH`. Clear
   the environment and set a fixed `PATH` of the trusted dirs before anything runs as root.
5. **All sources failing** currently exits 0 (unreachable today since AppImage always succeeds);
   make it exit 1 once more sources exist (M5).

## Smaller polish (any time)

- `fmt_size(1_048_575)` prints `1024.0 KiB`; installation printed via `Debug`; `doctor --json`
  carries errors in both `data.errors` and the envelope.
- CI: restrict `push` to `main` to avoid double runs; add top-level `permissions: contents: read`;
  the Arch job runs tests as root, which will matter for permission-sensitive M2 tests.
- Desktop `Exec` parsing recognises only a bare `env` prefix; single-layer unescaping.
- Test gaps: `--sort size`, non-ASCII column alignment, read-only trash, run0 with failing
  `systemctl`, `Installation` serialization.
