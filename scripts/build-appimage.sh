#!/bin/sh
# Builds dist/crumbs-<version>-x86_64.AppImage around the static binary
# (default: dist/crumbs-<version>-x86_64-linux-musl) and prints its path.
# Uses appimagetool from PATH, or the one named by APPIMAGETOOL; its default
# runtime is static, so the AppImage needs no libfuse2. APPIMAGE_RUNTIME
# names a runtime file to embed instead of the one appimagetool downloads
# (the release workflow pins one by checksum).
#
#   scripts/build-appimage.sh [static-binary]
set -eu
cd "$(dirname "$0")/.."
VERSION=$(cargo metadata --format-version 1 --no-deps --locked |
    python3 -c 'import json,sys; print(next(p["version"] for p in json.load(sys.stdin)["packages"] if p["name"] == "crumbs-cli"))')
BIN=${1:-dist/crumbs-$VERSION-x86_64-linux-musl}
TOOL=${APPIMAGETOOL:-appimagetool}
command -v "$TOOL" >/dev/null 2>&1 || {
    echo "error: appimagetool not found; install it from https://github.com/AppImage/appimagetool/releases or set APPIMAGETOOL" >&2
    exit 2
}
[ -f "$BIN" ] || { echo "error: $BIN not found; run scripts/build-static.sh first" >&2; exit 2; }

APPDIR=$HOME/.cache/crumbs-appdir/crumbs.AppDir
rm -rf "$APPDIR"
mkdir -p "$APPDIR/usr/bin" "$APPDIR/usr/share/metainfo" \
    "$APPDIR/usr/share/applications" "$APPDIR/usr/share/icons/hicolor/scalable/apps"
install -m 755 "$BIN" "$APPDIR/usr/bin/crumbs"
install -m 755 packaging/appimage/AppRun "$APPDIR/AppRun"
install -m 644 packaging/appimage/crumbs.desktop "$APPDIR/crumbs.desktop"
install -m 644 packaging/appimage/crumbs.desktop "$APPDIR/usr/share/applications/crumbs.desktop"
install -m 644 packaging/appimage/crumbs.svg "$APPDIR/crumbs.svg"
install -m 644 packaging/appimage/crumbs.svg "$APPDIR/usr/share/icons/hicolor/scalable/apps/crumbs.svg"
install -m 644 packaging/appimage/io.github.asafelobotomy.crumbs.metainfo.xml "$APPDIR/usr/share/metainfo/"
ln -s crumbs.svg "$APPDIR/.DirIcon"

mkdir -p dist
out=$PWD/dist/crumbs-$VERSION-x86_64.AppImage
rm -f "$out"
set -- --no-appstream
[ -n "${APPIMAGE_RUNTIME:-}" ] && set -- "$@" --runtime-file "$APPIMAGE_RUNTIME"
ARCH=x86_64 "$TOOL" "$@" "$APPDIR" "$out" >&2
echo "$out"
