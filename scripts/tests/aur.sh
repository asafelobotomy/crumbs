# The AUR packages: .SRCINFO in sync, namcap, and both build, install and
# run in an Arch container (as a non-root user: makepkg refuses root).
. "$(dirname "$0")/lib.sh"
AUR=$PWD/packaging/aur
WORK=$HOME/.cache/crumbs-test/aur
# The builds run as a container user, so their files are removed from
# inside podman's user namespace.
podman unshare rm -rf "$WORK"; mkdir -p "$WORK"
ARCH_IMAGE=docker.io/library/archlinux:latest

check aur_srcinfo_up_to_date
for pkg in crumbs crumbs-bin; do
    grep -q "^pkgver=$VERSION\$" "$AUR/$pkg/PKGBUILD" || fail "$pkg: pkgver is not $VERSION"
    want=$(cd "$AUR/$pkg" && makepkg --printsrcinfo) || fail "$pkg: makepkg --printsrcinfo failed"
    [ "$want" = "$(cat "$AUR/$pkg/.SRCINFO")" ] || fail "$pkg: .SRCINFO is stale (run makepkg --printsrcinfo > .SRCINFO)"
    grep -q "# filled in by scripts/release.sh" "$AUR/$pkg/PKGBUILD" || fail "$pkg: checksums not marked"
done

check aur_namcap_clean
if command -v namcap >/dev/null; then
    for pkg in crumbs crumbs-bin; do
        out=$(namcap "$AUR/$pkg/PKGBUILD" 2>&1)
        echo "$out"
        echo "$out" | grep -q ' E: ' && fail "$pkg: namcap: $(echo "$out" | grep ' E: ' | head -1)"
    done
fi

# Both builds run in one container: install the tools, build as `builder`,
# install with pacman -U, run, and namcap the built packages.
need "$STATIC"
mkdir -p "$WORK/crumbs" "$WORK/crumbs-bin"
cp "$AUR/crumbs/PKGBUILD" "$WORK/crumbs/"
git archive --format=tar.gz --prefix="crumbs-$VERSION/" -o "$WORK/crumbs/crumbs-$VERSION.tar.gz" HEAD
cp "$AUR/crumbs-bin/PKGBUILD" "$WORK/crumbs-bin/"
cp "$STATIC" "$WORK/crumbs-bin/crumbs-$VERSION"
cp LICENSE "$WORK/crumbs-bin/LICENSE"

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
        pacman -U --noconfirm "$pkg" >/dev/null
        chown -R root:root /work/'"$1"'
        crumbs --version
    ' 2>&1
}
out=$(run_build crumbs crumbs) || fail "$(echo "$out" | tail -5 | tr '\n' ' ')"
[ "$(echo "$out" | tail -1)" = "crumbs $VERSION" ] || fail "crumbs printed: $out"

check aur_bin_package_builds
out=$(run_build crumbs-bin crumbs-bin) || fail "$(echo "$out" | tail -5 | tr '\n' ' ')"
[ "$(echo "$out" | tail -1)" = "crumbs $VERSION" ] || fail "crumbs-bin printed: $out"

pass
