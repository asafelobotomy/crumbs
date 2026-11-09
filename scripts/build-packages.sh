#!/bin/sh
# Builds dist/crumbs_<version>_amd64.deb and dist/crumbs-<version>-1.x86_64.rpm
# around the static binary (default: dist/crumbs-<version>-x86_64-linux-musl)
# and prints both paths. cargo-deb and cargo-generate-rpm are installed into
# ~/.cache/crumbs-tools when missing (build-time only).
#
#   scripts/build-packages.sh [static-binary]
set -eu
cd "$(dirname "$0")/.."
TARGET=x86_64-unknown-linux-musl
VERSION=$(cargo metadata --format-version 1 --no-deps --locked |
    python3 -c 'import json,sys; print(next(p["version"] for p in json.load(sys.stdin)["packages"] if p["name"] == "crumbs-cli"))')
BIN=${1:-dist/crumbs-$VERSION-x86_64-linux-musl}
[ -f "$BIN" ] || { echo "error: $BIN not found; run scripts/build-static.sh first" >&2; exit 2; }

TOOLS=${CRUMBS_TOOLS:-$HOME/.cache/crumbs-tools}
export PATH="$TOOLS/bin:$PATH"
for tool in cargo-deb cargo-generate-rpm; do
    command -v "$tool" >/dev/null 2>&1 ||
        CARGO_TARGET_DIR="$TOOLS/target" cargo install --locked --root "$TOOLS" "$tool" >&2
done

# Both tools take the binary from the target directory's release folder.
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
built_deb=$(cargo deb -p crumbs-cli --no-build --no-strip --target "$TARGET" --output "$deb" 2>&1 | tail -1) || {
    echo "$built_deb" >&2; exit 1; }
[ -f "$deb" ] || { echo "error: cargo deb wrote $built_deb, not $deb" >&2; exit 1; }
cargo generate-rpm -p crates/crumbs-cli --target "$TARGET" -o "$rpm" >&2
echo "$deb"
echo "$rpm"
