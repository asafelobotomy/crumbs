# M6 follow-ups (packaging)

M6 left these out on purpose, or noticed them on the way.

## Not done yet

1. **Signing.** Releases carry `SHA256SUMS` but no signature. Sign `SHA256SUMS` (minisign or GPG) in the release workflow, and give the AUR `crumbs-bin` a `validpgpkeys`.
2. **aarch64.** Every artifact is x86_64. Add `aarch64-unknown-linux-musl` once x86_64 releases are stable (spec §12); the AppImage needs the aarch64 runtime and the AUR `crumbs-bin` a `source_aarch64`.
3. **AUR publishing.** Publishing needs the maintainer's AUR account, so it is a manual step (README, "Releasing").
4. **The release workflow has never run.** The repo has no GitHub remote yet. `scripts/release.sh` ran locally, and `scripts/test-packaging.sh` checked its output, but `.github/workflows/release.yml` has not run on a real tag.

## Known limits

5. **appimagetool is unpinned.** (cargo-deb is pinned in `scripts/build-packages.sh`.) The release workflow downloads appimagetool's `continuous` build, which also fetches the AppImage runtime at build time. Pin both by version and checksum.
6. **lintian warnings on the `.deb`.** It has no errors (the packaging test runs `lintian --fail-on error`), but it warns about five things:
   - `empty-field Depends`: cargo-deb 3.8 always writes the field, and dpkg drops it on install;
   - `no-manual-page`: crumbs has no man page yet;
   - `wrong-name-for-changelog-of-native-package`: the version has no Debian revision, so lintian expects `changelog.gz`;
   - `shared-library-lacks-prerequisites` and `uses-dpkg-database-directly`: false positives for a static-pie binary that reads `/var/lib/dpkg/status` on purpose.
7. **The `.rpm` is built with `rpmbuild`,** from `packaging/rpm/crumbs.spec`, not cargo-generate-rpm: cargo-generate-rpm 0.21.1 cannot own directories, so `rpm -e` left `/usr/share/doc/crumbs` and `/usr/share/licenses/crumbs` behind.
8. **rpm self-removal is not tested.** In Debian, the packaging test removes the `.deb` with `crumbs uninstall dpkg:crumbs`. In Fedora it uses `rpm -e`, because dnf's simulation needs repository metadata and the test container has no network.
9. **The source AUR package tests in the debug profile.** `check()` runs `cargo test`, not `cargo test --release`, because the CLI tests rely on debug-only fixture roots.
10. **The maintainer address** in the `.deb`, the AUR files and `packaging/deb/copyright` is `asafelobotomy@users.noreply.github.com`, a placeholder that receives no mail. Replace it with a real contact address before publishing.
