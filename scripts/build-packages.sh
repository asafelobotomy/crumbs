#!/bin/sh
# Builds dist/crumbs_<version>_amd64.deb (cargo-deb) and
# dist/crumbs-<version>-1.x86_64.rpm (rpmbuild, packaging/rpm/crumbs.spec)
# around the static binary (default: dist/crumbs-<version>-x86_64-linux-musl)
# and prints both paths. cargo-deb is installed into ~/.cache/crumbs-tools
# when missing (build-time only); rpmbuild must be installed.
#
#   scripts/build-packages.sh [static-binary]
set -eu
cd "$(dirname "$0")/.."
TARGET=x86_64-unknown-linux-musl
VERSION=$(cargo metadata --format-version 1 --no-deps --locked |
    python3 -c 'import json,sys; print(next(p["version"] for p in json.load(sys.stdin)["packages"] if p["name"] == "crumbs-cli"))')
BIN=${1:-dist/crumbs-$VERSION-x86_64-linux-musl}
[ -f "$BIN" ] || { echo "error: $BIN not found; run scripts/build-static.sh first" >&2; exit 2; }

# Pinned: a tag build installs these fresh, and must build the same package
# that scripts/test-packaging.sh checked.
CARGO_DEB_VERSION=3.8.0
TOOLS=${CRUMBS_TOOLS:-$HOME/.cache/crumbs-tools}
export PATH="$TOOLS/bin:$PATH"
ensure_tool() { # <crate> <version>
    if ! "$1" --version 2>/dev/null | grep -qx "$1 $2"; then
        CARGO_TARGET_DIR="$TOOLS/target" cargo install --locked --force --root "$TOOLS" --version "$2" "$1" >&2
    fi
}
ensure_tool cargo-deb "$CARGO_DEB_VERSION"
command -v rpmbuild >/dev/null 2>&1 || {
    echo "error: rpmbuild not found; install rpm-tools (Arch), rpm-build (Fedora) or rpm (Debian)" >&2
    exit 2
}

# cargo-deb takes the binary from the target directory's release folder.
TARGET_DIR=$(cargo metadata --format-version 1 --no-deps --locked |
    python3 -c 'import json,sys; print(json.load(sys.stdin)["target_directory"])')
staged=$TARGET_DIR/$TARGET/release/crumbs
mkdir -p "$(dirname "$staged")"
if ! [ "$BIN" -ef "$staged" ]; then
    cp "$BIN" "$staged"
fi

mkdir -p dist
deb=$PWD/dist/crumbs_${VERSION}_amd64.deb
rpm=$PWD/dist/crumbs-$VERSION-1.x86_64.rpm
rm -f "$deb" "$rpm"
log=$HOME/.cache/crumbs-packages.log
cargo deb -p crumbs-cli --no-build --no-strip --target "$TARGET" --output "$deb" >"$log" 2>&1 || {
    cat "$log" >&2; exit 1; }
[ -f "$deb" ] || { cat "$log" >&2; echo "error: cargo deb did not write $deb" >&2; exit 1; }

RPMTOP=$HOME/.cache/crumbs-rpmbuild
rm -rf "$RPMTOP"
mkdir -p "$RPMTOP/src"
cp "$BIN" "$RPMTOP/src/crumbs"
cp README.md LICENSE "$RPMTOP/src/"
rpmbuild -bb packaging/rpm/crumbs.spec --target x86_64 \
    --define "_topdir $RPMTOP" --define "crumbs_version $VERSION" --define "crumbs_src $RPMTOP/src" \
    >"$log" 2>&1 || { cat "$log" >&2; exit 1; }
cp "$RPMTOP/RPMS/x86_64/crumbs-$VERSION-1.x86_64.rpm" "$rpm"
echo "$deb"
echo "$rpm"
