# shellcheck shell=sh
# The AUR packages: .SRCINFO in sync, namcap, and both build, install and
# run in an Arch container (as a non-root user: makepkg refuses root).
. "$(dirname "$0")/lib.sh"
AUR=$PWD/packaging/aur
# The files the uq/ folder should put under usr/share/uq: its launcher action and every file of
# the Settings page (not uq/README.md). uq/pages/<id>/ goes to settings/pages/<id>/.
uq_files() {
    (cd uq && find launcher pages -type f | sort) |
        sed -e 's|^launcher/|usr/share/uq/launcher/|' -e 's|^pages/|usr/share/uq/settings/pages/|' | sort
}
WORK=$HOME/.cache/crumbs-test/aur
# The builds run as a container user, so their files are removed from
# inside podman's user namespace.
podman unshare rm -rf "$WORK"; mkdir -p "$WORK"
ARCH_IMAGE=docker.io/library/archlinux:latest

check aur_srcinfo_up_to_date
for pkg in crum-bs crum-bs-bin; do
    grep -q "^pkgver=$VERSION\$" "$AUR/$pkg/PKGBUILD" || fail "$pkg: pkgver is not $VERSION"
    want=$(cd "$AUR/$pkg" && makepkg --printsrcinfo) || fail "$pkg: makepkg --printsrcinfo failed"
    [ "$want" = "$(cat "$AUR/$pkg/.SRCINFO")" ] || fail "$pkg: .SRCINFO is stale (run makepkg --printsrcinfo > .SRCINFO)"
    grep -q "# filled in by scripts/release.sh" "$AUR/$pkg/PKGBUILD" || fail "$pkg: checksums not marked"
    # Every downloaded file is named after the version, so a shared SRCDEST
    # never mixes versions.
    unversioned=$(grep -E '^\s+source(_x86_64)? = ' "$AUR/$pkg/.SRCINFO" | grep -v "$VERSION" || true)
    [ -z "$unversioned" ] || fail "$pkg: source names without the version: $unversioned"
    unversioned=$(grep -E '^\s+source(_x86_64)? = [^:]+::' "$AUR/$pkg/.SRCINFO" | sed 's/::.*//' | grep -v "$VERSION" || true)
    [ -z "$unversioned" ] || fail "$pkg: local source names without the version: $unversioned"
done

check aur_namcap_clean
if command -v namcap >/dev/null; then
    for pkg in crum-bs crum-bs-bin; do
        out=$(namcap "$AUR/$pkg/PKGBUILD" 2>&1)
        echo "$out"
        echo "$out" | grep -q ' E: ' && fail "$pkg: namcap: $(echo "$out" | grep ' E: ' | head -1)"
    done
fi

# Both builds run in one container: install the tools, build as `builder`,
# install with pacman -U, run, and namcap the built packages.
need "$STATIC"
mkdir -p "$WORK/crum-bs" "$WORK/crum-bs-bin"
cp "$AUR/crum-bs/PKGBUILD" "$WORK/crum-bs/"
git archive --format=tar.gz --prefix="crumbs-$VERSION/" -o "$WORK/crum-bs/crumbs-$VERSION.tar.gz" HEAD
cp "$AUR/crum-bs-bin/PKGBUILD" "$WORK/crum-bs-bin/"
# crum-bs-bin downloads the same source tarball (it holds the licence and the uQ files) and the
# binary, which keeps the name the release gives it.
cp "$WORK/crum-bs/crumbs-$VERSION.tar.gz" "$WORK/crum-bs-bin/"
cp "$STATIC" "$WORK/crum-bs-bin/"

check aur_source_package_builds
run_build() { # <pkg dir> <package name>
    podman run --rm -v "$WORK:/work" -v crumbs-aur-cargo:/cargo "$ARCH_IMAGE" sh -euc '
        pacman -Sy --noconfirm --needed base-devel rust namcap >/dev/null 2>&1
        useradd -m builder 2>/dev/null || true
        chown -R builder /work/'"$1"' /cargo
        su builder -c "cd /work/'"$1"' && CARGO_HOME=/cargo makepkg --skipchecksums --noconfirm" >/work/'"$1"'.makepkg.log 2>&1 ||
            { tail -30 /work/'"$1"'.makepkg.log; exit 1; }
        pkg=$(ls /work/'"$1"'/'"$2"'-'"$VERSION"'-*-x86_64.pkg.tar.zst | grep -v -- -debug-)
        namcap "$pkg" | grep " E: " && exit 1
        # What the package puts under /usr/share/uq, one file per line.
        bsdtar -tf "$pkg" | grep "^usr/share/uq/" | grep -v "/\$" | sort >/work/'"$1"'.uqfiles
        pacman -U --noconfirm "$pkg" >/dev/null
        chown -R root:root /work/'"$1"'
        crumbs --version
    ' 2>&1
}
out=$(run_build crum-bs crum-bs) || fail "$(echo "$out" | tail -5 | tr '\n' ' ')"
[ "$(echo "$out" | tail -1)" = "crumbs $VERSION" ] || fail "crum-bs printed: $out"
check aur_source_package_installs_the_uq_folder
[ "$(cat "$WORK/crum-bs.uqfiles")" = "$(uq_files)" ] || fail "crum-bs installs under usr/share/uq: $(tr '\n' ' ' < "$WORK/crum-bs.uqfiles"); want: $(uq_files | tr '\n' ' ')"

check aur_bin_package_builds
out=$(run_build crum-bs-bin crum-bs-bin) || fail "$(echo "$out" | tail -5 | tr '\n' ' ')"
[ "$(echo "$out" | tail -1)" = "crumbs $VERSION" ] || fail "crum-bs-bin printed: $out"
check aur_bin_package_installs_the_uq_folder
[ "$(cat "$WORK/crum-bs-bin.uqfiles")" = "$(uq_files)" ] || fail "crum-bs-bin installs under usr/share/uq: $(tr '\n' ' ' < "$WORK/crum-bs-bin.uqfiles"); want: $(uq_files | tr '\n' ' ')"

pass
