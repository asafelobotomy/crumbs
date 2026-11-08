# M4 review follow-ups

These items came out of the M4 per-task reviews and the final whole-branch review. Each one either fails safe today or only affects what the tool reports. Larger deferred scope, such as system leftovers and removing unmanaged apps, is listed in [m4b-scope.md](m4b-scope.md).

## Fails safe, but confusing

1. **Overlapping pacman closures.** If you uninstall two pacman apps and one is already in the other's closure, the second `-Rn` fails with "target not found". The post-check reports this truthfully, but the message is unclear. Fix: merge the closures of all pacman plans in a run, or drop names already removed.
2. **Shell killed during step 0.** If the shell is killed before it emits any marker, the run is reported as a denial (exit 5, "nothing … was changed"), even though step 0 may have partly run.
3. **Truncated comm matching.** `comm` is cut to 15 bytes, so long app names that share a prefix (`visual-studio-code` vs `-insiders`) can make one app look like it is running. You get an extra refusal, which `--force` overrides. Fix: cross-check argv0 or the exe path when the name is longer than 15 bytes.
4. **Cross-plan dependency covers.** A dependent only records the first earlier action that covers it.
5. **Privileged dependents.** A privileged action that carries `depends_on` or `depends_on_plan` is skipped, conservatively. No current flow produces one.

## Preview accuracy

6. **Journal size.** The estimate (`usage − journal_max`) counts active journal files, which vacuum never removes, so it can be too high.
7. **`flatpak-unused-system`.** It is offered whenever any system runtime exists, not only when one is unused. It is opt-in, and its size is unknown.
8. **`pkg_cache_keep = 0`.** This drops the cached packages for installed versions too. Add a note when the config is loaded.
9. **Flatpak post-check.** System and custom flatpak removals are not post-checked the way pacman removals are.

## Robustness

10. **Batch errors.** One batch error, such as TooLarge, fails every plan's privileged actions in the run.
11. **Partial test runner.** The debug `partial:<n>` test runner finds step lines by their text form, so it is tied to the batch format.
12. **Process descendants.** A daemonised descendant of a privileged step can hold the batch until the timeout. A `UserCommand` can outlive crumbs, and Ctrl-C reaches only crumbs' own process group.
13. **Root mismatch.** `SystemPrivilegedRunner` guards its own root, not the `Env`'s.

## Tests

14. **Cross-plan skip.** No end-to-end CLI test covers a cross-plan skip. Only core and `drop_overlaps` unit tests cover it.
15. **Missing /proc.** No test covers history mode-tightening when `/proc` is missing. The code warns instead of panicking.
