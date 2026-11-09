#!/bin/sh
# Builds the release binary for x86_64-unknown-linux-musl (fully static) and
# copies it to dist/crumbs-<version>-x86_64-linux-musl, printing that path.
# Every package (AppImage, .deb, .rpm, crum-bs-bin) wraps this one binary.
set -eu
cd "$(dirname "$0")/.."
TARGET=x86_64-unknown-linux-musl
VERSION=$(cargo metadata --format-version 1 --no-deps --locked |
    python3 -c 'import json,sys; print(next(p["version"] for p in json.load(sys.stdin)["packages"] if p["name"] == "crumbs-cli"))')
if ! rustup target list --installed | grep -qx "$TARGET"; then
    rustup target add "$TARGET" >&2
fi
cargo build --release --locked --target "$TARGET" -p crumbs-cli >&2
TARGET_DIR=$(cargo metadata --format-version 1 --no-deps --locked |
    python3 -c 'import json,sys; print(json.load(sys.stdin)["target_directory"])')
mkdir -p dist
out=dist/crumbs-$VERSION-x86_64-linux-musl
cp "$TARGET_DIR/$TARGET/release/crumbs" "$out"
chmod 755 "$out"
echo "$PWD/$out"
