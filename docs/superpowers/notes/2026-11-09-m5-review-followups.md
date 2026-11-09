# M5 follow-ups (cross-distro)

M5 left these out on purpose, or noticed them on the way. None of them is a known way to lose data.

## Not supported yet

1. **zypper (openSUSE) and plain `rpm -e`.** rpm systems without `dnf` can list apps and use them for ownership, but cannot remove packages: the removal is refused because `dnf is not installed`.
2. **`dpkg -P` when `apt-get` is missing.** Debian systems without apt (rare) cannot remove packages: the removal is refused because `apt-get is not installed`.
3. **A real dnf4 system.** The dnf4 table parser is tested only against a hand-written fixture in dnf4's documented format. The matrix's Fedora image ships dnf5.
4. **A real snapd.** snapd needs systemd, so Snap listing and removal are tested only against a hand-written `snap list` fixture.

## Known limits

(The final review fixed the exact-list removal: root now runs `dpkg --purge` and `rpm -e`, which refuse rather than widen.)

5. **rpm file names that are not valid UTF-8.** The `CommandRunner` returns text, so such a name fails the whole `rpm -qa` query. That disables rpm listing and system leftovers until the runner can return bytes. This fails closed.
6. **Post-checks for snap removals.** Like system Flatpak removals (an M4 follow-up), these are judged by `snap`'s own exit status. `snap_still_installed` exists, but the executor post-checks only `package-remove` actions.
7. **dnf orphan size.** The size of `orphan-pkgs` on dnf is unknown: dnf's table sizes are rounded and use mixed units.
8. **Snap app sizes.** Snaps report no install size.
9. **Root-only test skips.** When the tests run as root (as in the containers), tests that assume a normal user skip: temp cleanup, the unreadable-directory premise and the scan snapshot. Escalation-detection tests pin a normal user's uid. The non-root CI job covers the skipped paths.

## The CI workflow

10. **Not yet run on GitHub.** `.github/workflows/ci.yml` gained a `distro` matrix job running `scripts/distro-check.sh --inside`, but it has never run there, because the repo has no remote. The same script passed locally with podman for archlinux, debian:trixie, ubuntu and fedora.
11. **Network.** The script installs rustup and the build tools in each container, so it needs network. The toolchain is cached in podman volumes (`crumbs-cargo-*`, `crumbs-rustup-*`, `crumbs-target-*`).

## Deferred from the final review (minor)

12. **Root preview wording.** A root run's preview still says "you will be asked once", though nothing asks.
13. **Misplaced doc comment.** The comment for `not_installed` in `bloat/system/mod.rs` now sits on `PackageManager`.
14. **Mixed systems.** `pkg-cache` and `orphan-pkgs` pick the first installed tool (pacman, then apt-get, then dnf), while ownership combines every database.
15. **dpkg multiarch.** A package installed for two architectures becomes two apps with the same id, `dpkg:<name>`.
16. **Content snaps.** Snaps such as `gtk-common-themes` are listed as GUI apps.
17. **dnf multilib.** `foo.x86_64` and `foo.i686` show as one package with one version, and both are removed.
18. **Dead code.** The snap branch of the reinstall hint is unreachable: snaps are a `privileged-command`, not a `package-remove`.
19. **History source values.** A history line with an unknown future `source` value fails to deserialize as a whole.
20. **Root test skips.** Some tests could assert the root behaviour instead of skipping, for example doctor's "running as root".
21. **Half-installed fixture.** The dpkg fixture has no half-installed stanza.
