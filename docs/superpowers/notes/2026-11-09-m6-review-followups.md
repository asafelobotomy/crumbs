# M6 follow-ups (packaging)

M6 left these out on purpose, or noticed them on the way.

## Not done yet

1. **Signing.** Releases carry `SHA256SUMS` but no signature. Sign `SHA256SUMS` (minisign or GPG) in the release workflow, and give the AUR `crumbs-bin` a `validpgpkeys`.
2. **aarch64.** Every artifact is x86_64. Add `aarch64-unknown-linux-musl` once x86_64 releases are stable (spec §12); the AppImage needs the aarch64 runtime and the AUR `crumbs-bin` a `source_aarch64`.
3. **AUR publishing.** Publishing needs the maintainer's AUR account, so it is a manual step (README, "Releasing").
4. **The release workflow has never run.** The repo has no GitHub remote yet. `scripts/release.sh` ran locally, and `scripts/test-packaging.sh` checked its output, but `.github/workflows/release.yml` has not run on a real tag.

## Known limits

5. **appimagetool is unpinned.** The release workflow downloads appimagetool's `continuous` build, which also fetches the AppImage runtime at build time. Pin both by version and checksum.
6. **The `crumbs` AUR checksum.** `release.sh` sums `git archive` at HEAD. That matches GitHub's tag tarball only when the tag points at the same commit and GitHub's gzip output is unchanged; re-check after tagging (README, "Releasing").
7. **The `.deb` has an empty `Depends:` field.** cargo-deb 3.8 always writes it. dpkg accepts it and drops it on install; lintian may warn.
8. **The `.rpm` has no Group, and its description is the summary.** cargo-generate-rpm supports neither a package group nor a separate long description.
9. **rpm self-removal is not tested.** In Debian, the packaging test removes the `.deb` with `crumbs uninstall dpkg:crumbs`. In Fedora it uses `rpm -e`, because dnf's simulation needs repository metadata and the test container has no network.
10. **The source AUR package tests in the debug profile.** `check()` runs `cargo test`, not `cargo test --release`, because the CLI tests rely on debug-only fixture roots.
