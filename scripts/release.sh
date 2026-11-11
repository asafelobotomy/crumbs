#!/bin/sh
# Builds every release artifact into dist/ and writes dist/SHA256SUMS, the
# release notes, and the AUR files with real checksums under dist/aur/.
# It never tags, pushes or uploads: those steps are yours (see README).
#
#   scripts/release.sh <version> [--allow-dirty]
#
# <version> must equal the workspace version, CHANGELOG.md must have a
# `## <version>` section, and the working tree must be clean (--allow-dirty
# is for tests).
set -eu
cd "$(dirname "$0")/.."

die() { echo "error: $1" >&2; exit 2; }

want=
allow_dirty=0
for arg in "$@"; do
    case "$arg" in
        --allow-dirty) allow_dirty=1 ;;
        -*) die "unknown option $arg" ;;
        *) [ -z "$want" ] || die "more than one version given"; want=$arg ;;
    esac
done
[ -n "$want" ] || die "usage: scripts/release.sh <version> [--allow-dirty]"

VERSION=$(cargo metadata --format-version 1 --no-deps --locked |
    python3 -c 'import json,sys; print(next(p["version"] for p in json.load(sys.stdin)["packages"] if p["name"] == "crumbs-cli"))')
[ "$want" = "$VERSION" ] || die "version $want does not match Cargo.toml ($VERSION)"
if [ "$allow_dirty" -eq 0 ] && [ -n "$(git status --porcelain)" ]; then
    echo "refusing: the working tree has uncommitted changes" >&2
    exit 2
fi
grep -q "^## $VERSION\$" CHANGELOG.md || die "CHANGELOG.md has no '## $VERSION' section"

# Files that name the version themselves must agree before anything builds.
named() { # <file> <sed expression printing the version it names>
    found=$(sed -n "$2" "$1" | head -1)
    [ "$found" = "$VERSION" ] || die "$1 names version ${found:-none}, not $VERSION"
}
for pkg in crum-bs crum-bs-bin; do
    named "packaging/aur/$pkg/PKGBUILD" 's/^pkgver=//p'
    named "packaging/aur/$pkg/.SRCINFO" 's/^\tpkgver = //p'
done
named packaging/appimage/io.github.asafelobotomy.crumbs.metainfo.xml 's/.*<release version="\([^"]*\)".*/\1/p'
named packaging/deb/changelog 's/^crumbs (\([^)]*\)).*/\1/p'

scripts/check-licenses.sh >&2 || die "a dependency's licence is not allowed (listed above)"

mkdir -p dist
rm -rf dist/aur
rm -f dist/SHA256SUMS dist/RELEASE_NOTES.md dist/LICENSE

BIN=$(scripts/build-static.sh)
APPIMAGE=$(scripts/build-appimage.sh "$BIN")
scripts/build-packages.sh "$BIN" >/dev/null
DEB=crumbs_${VERSION}_amd64.deb
RPM=crumbs-$VERSION-1.x86_64.rpm
cp LICENSE dist/LICENSE
# The source tarball the AUR `crumbs` package downloads. It is uploaded
# with the release, so its sum is of the bytes users fetch; GitHub's own
# tag tarballs are compressed differently and would not match.
TARBALL=dist/crumbs-$VERSION.tar.gz
git archive --format=tar.gz --prefix="crumbs-$VERSION/" -o "$TARBALL" HEAD

(
    cd dist
    sha256sum "$(basename "$BIN")" "$(basename "$APPIMAGE")" "$DEB" "$RPM" \
        "$(basename "$TARBALL")" LICENSE > SHA256SUMS
)

# The release notes: the changelog section, without its heading.
python3 - "$VERSION" > dist/RELEASE_NOTES.md <<'PY'
import sys
version = sys.argv[1]
out, inside = [], False
for line in open("CHANGELOG.md"):
    if line.startswith("## "):
        if inside:
            break
        inside = line.strip() == "## " + version
        continue
    if inside:
        out.append(line)
sys.stdout.write("".join(out).strip() + "\n")
PY

# The AUR files, with each 'SKIP' replaced by the artifact's sum.
sum() { sha256sum | cut -d' ' -f1; }
tarball_sum=$(sum < "$TARBALL")
bin_sum=$(sum < "$BIN")
fill() { # <package> <array>=<sum>...
    pkg=$1; shift
    mkdir -p "dist/aur/$pkg"
    python3 - "packaging/aur/$pkg" "dist/aur/$pkg" "$@" <<'PY'
import re, sys
src, dst, pairs = sys.argv[1], sys.argv[2], sys.argv[3:]
sums = dict(p.split("=", 1) for p in pairs)
pkgbuild = open(src + "/PKGBUILD").read()
srcinfo = open(src + "/.SRCINFO").read()
for array, value in sums.items():
    pkgbuild, n = re.subn(r"^%s=\('SKIP'\).*$" % array, "%s=('%s')" % (array, value), pkgbuild, flags=re.M)
    assert n == 1, "PKGBUILD: %s=('SKIP') found %d times" % (array, n)
    srcinfo, n = re.subn(r"^\t%s = SKIP$" % array, "\t%s = %s" % (array, value), srcinfo, flags=re.M)
    assert n == 1, ".SRCINFO: %s = SKIP found %d times" % (array, n)
assert "SKIP" not in pkgbuild and "SKIP" not in srcinfo, "a SKIP is left in " + src
open(dst + "/PKGBUILD", "w").write(pkgbuild)
open(dst + "/.SRCINFO", "w").write(srcinfo)
PY
}
fill crum-bs "sha256sums=$tarball_sum"
# The -bin package takes its licence and uQ files from the same source tarball.
fill crum-bs-bin "sha256sums=$tarball_sum" "sha256sums_x86_64=$bin_sum"

echo "dist/ now holds:" >&2
(cd dist && sha256sum -c SHA256SUMS >&2)
