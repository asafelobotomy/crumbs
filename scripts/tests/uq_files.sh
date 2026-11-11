# shellcheck shell=sh
# What the AUR packages install under /usr/share/uq equals the uq/ folder (its launcher action
# and every file of the Settings page). This runs each PKGBUILD's package() in a scratch tree:
# no makepkg or container needed, so CI can run it too. scripts/tests/aur.sh checks the same of
# the built packages.
. "$(dirname "$0")/lib.sh"
SCRATCH=$HOME/.cache/crumbs-test/uq-files
rm -rf "$SCRATCH"

want() {
    (cd uq && find launcher pages -type f | sort) |
        sed -e 's|^launcher/|usr/share/uq/launcher/|' -e 's|^pages/|usr/share/uq/settings/pages/|' | sort
}
[ -n "$(want)" ] || fail "uq/ holds no files"

for pkg in crum-bs crum-bs-bin; do
    check "${pkg}_package_installs_the_uq_folder"
    src=$SCRATCH/$pkg/src
    out=$SCRATCH/$pkg/pkg
    mkdir -p "$src/crumbs-$VERSION" "$out"
    cp -r LICENSE README.md uq "$src/crumbs-$VERSION/"
    # What the build leaves for package(): the release binary, or the downloaded static one.
    mkdir -p "$src/crumbs-$VERSION/target/release"
    echo '#!/bin/sh' > "$src/crumbs-$VERSION/target/release/crumbs"
    echo '#!/bin/sh' > "$src/crumbs-$VERSION-x86_64-linux-musl"
    # shellcheck disable=SC2016 # $srcdir and $pkgdir are expanded by the inner shell
    bash -euc 'cd "$1"; srcdir=$1; pkgdir=$2; CARCH=x86_64; . "$3"; package' _ "$src" "$out" "$PWD/packaging/aur/$pkg/PKGBUILD" ||
        fail "$pkg: package() failed"
    got=$(cd "$out" && find usr/share/uq -type f | sort)
    [ "$got" = "$(want)" ] || fail "$pkg installs under usr/share/uq: $(echo "$got" | tr '\n' ' '); want: $(want | tr '\n' ' ')"
    [ -f "$out/usr/bin/crumbs" ] || fail "$pkg: no /usr/bin/crumbs"
    [ -f "$out/usr/share/licenses/$pkg/LICENSE" ] || fail "$pkg: no licence"
done

rm -rf "$SCRATCH"
pass
