# The AppImage: contents, desktop entry, runs without libfuse2 or /dev/fuse,
# works from a path with spaces and quotes, and keeps MCP stdout pure.
. "$(dirname "$0")/lib.sh"
APPIMAGE=$DIST/crumbs-$VERSION-x86_64.AppImage
WORK=$HOME/.cache/crumbs-test/appimage
rm -rf "$WORK"; mkdir -p "$WORK"

check appimage_lists_expected_files
need "$APPIMAGE"
(cd "$WORK" && "$APPIMAGE" --appimage-extract >/dev/null) || fail "--appimage-extract failed"
root=$WORK/squashfs-root
[ -x "$root/AppRun" ] || fail "AppRun missing or not executable"
for f in usr/bin/crumbs crumbs.desktop crumbs.svg .DirIcon \
    usr/share/metainfo/io.github.asafelobotomy.crumbs.metainfo.xml; do
    [ -e "$root/$f" ] || fail "missing $f"
done
[ -x "$root/usr/bin/crumbs" ] || fail "usr/bin/crumbs not executable"

check appimage_desktop_entry_is_valid
desktop=$root/crumbs.desktop
if command -v desktop-file-validate >/dev/null; then
    out=$(desktop-file-validate "$desktop" 2>&1) || fail "desktop-file-validate: $out"
fi
for key in 'Type=Application' 'Name=crumBS' 'Exec=crumbs' 'Icon=crumbs' 'Terminal=true'; do
    grep -qx "$key" "$desktop" || fail "crumbs.desktop lacks $key"
done
if command -v appstreamcli >/dev/null; then
    out=$(appstreamcli validate --no-net --pedantic "$root/usr/share/metainfo/io.github.asafelobotomy.crumbs.metainfo.xml" 2>&1) ||
        fail "appstreamcli: $out"
fi

check appimage_runs_without_libfuse2
if readelf -d "$APPIMAGE" 2>/dev/null | grep -q NEEDED; then
    fail "the runtime is dynamically linked: $(readelf -d "$APPIMAGE" | grep NEEDED | tr -s ' ' | tr '\n' ';')"
fi
img=docker.io/library/debian:trixie
name=/dist/$(basename "$APPIMAGE")
out=$(podman run --rm --network=none -v "$DIST:/dist:ro" -e APPIMAGE_EXTRACT_AND_RUN=1 "$img" "$name" --version 2>&1) ||
    fail "APPIMAGE_EXTRACT_AND_RUN=1 in $img: $out"
[ "$out" = "crumbs $VERSION" ] || fail "APPIMAGE_EXTRACT_AND_RUN=1 printed: $out"
out=$(podman run --rm --network=none -v "$DIST:/dist:ro" "$img" "$name" --appimage-extract-and-run --version 2>&1) ||
    fail "--appimage-extract-and-run in $img: $out"
[ "$out" = "crumbs $VERSION" ] || fail "--appimage-extract-and-run printed: $out"

check appimage_in_path_with_spaces
dir="$WORK/My Apps"
mkdir -p "$dir"
odd="$dir/crumbs's app.AppImage"
cp "$APPIMAGE" "$odd"
out=$("$odd" --version 2>&1) || fail "--version from a path with spaces: $out"
[ "$out" = "crumbs $VERSION" ] || fail "--version printed: $out"
st=0; err=$("$odd" apps show "a b" 2>&1 >/dev/null) || st=$?
[ "$st" -eq 2 ] || fail "apps show 'a b': exit $st: $err"
echo "$err" | grep -q "'a b'" || fail "the argument was split: $err"

check appimage_mcp_stdout_is_protocol_only
out=$(APPIMAGE_EXTRACT_AND_RUN=1 python3 -I scripts/tests/mcp_stdout.py "$odd" mcp 2>&1) || fail "$out"
echo "$out"

rm -rf "$WORK"
pass
