# The .deb and .rpm: metadata, and install / run / remove in a container.
. "$(dirname "$0")/lib.sh"
DEB=$DIST/crumbs_${VERSION}_amd64.deb
RPM=$DIST/crumbs-$VERSION-1.x86_64.rpm
URL=https://github.com/asafelobotomy/crumbs

check deb_metadata
need "$DEB"
info=$(dpkg-deb --info "$DEB")
echo "$info" | grep -q '^ Package: crumbs$' || fail "no Package: crumbs"
echo "$info" | grep -q "^ Version: $VERSION\$" || fail "no Version: $VERSION"
echo "$info" | grep -q '^ Section: utils$' || fail "no Section: utils"
# cargo-deb always writes the field; empty, dpkg drops it on install.
echo "$info" | grep -Eq '^ Depends: *[^ ]' && fail "has dependencies: $(echo "$info" | grep '^ Depends:')"
contents=$(dpkg-deb --contents "$DEB")
for f in ./usr/bin/crumbs ./usr/share/doc/crumbs/README.md ./usr/share/doc/crumbs/copyright; do
    echo "$contents" | grep -q " $f\$" || fail "the .deb lacks $f"
done

check rpm_metadata
need "$RPM"
info=$(rpm -qip "$RPM" 2>/dev/null)
echo "$info" | grep -Eq '^Name +: crumbs$' || fail "no Name: crumbs"
echo "$info" | grep -Eq "^Version +: $VERSION\$" || fail "no Version: $VERSION"
echo "$info" | grep -Eq '^License +: GPL-3.0-or-later$' || fail "no License: GPL-3.0-or-later"
echo "$info" | grep -Eq "^URL +: $URL\$" || fail "no URL: $URL"
requires=$(rpm -qp --requires "$RPM" 2>/dev/null | grep -v '^rpmlib(' || true)
[ -z "$requires" ] || fail "the .rpm requires: $requires"
files=$(rpm -qlp "$RPM" 2>/dev/null)
for f in /usr/bin/crumbs /usr/share/doc/crumbs/README.md /usr/share/licenses/crumbs/LICENSE; do
    echo "$files" | grep -qx "$f" || fail "the .rpm lacks $f"
done

check deb_and_rpm_install_run_and_remove_cleanly
# Inside each container: install, run, list itself, remove, and check that
# every file the package installed is gone. Debian removes it through
# `crumbs uninstall` itself; Fedora with `rpm -e`.
out=$(podman run --rm --network=none -v "$DIST:/dist:ro" docker.io/library/debian:trixie sh -euc '
    dpkg -i /dist/'"$(basename "$DEB")"' >/dev/null
    files=$(dpkg -L crumbs | while read -r f; do [ -f "$f" ] && echo "$f"; done)
    [ -n "$files" ] || { echo "dpkg -L lists no files"; exit 1; }
    [ "$(crumbs --version)" = "crumbs '"$VERSION"'" ] || { echo "bad --version"; exit 1; }
    crumbs apps list --all --json | grep -q "\"dpkg:crumbs\"" || { echo "apps list lacks dpkg:crumbs"; exit 1; }
    crumbs uninstall dpkg:crumbs --yes --json >/tmp/u.json || { echo "uninstall failed: $(cat /tmp/u.json)"; exit 1; }
    dpkg -s crumbs >/dev/null 2>&1 && { echo "still installed after crumbs uninstall"; exit 1; }
    for f in $files; do [ ! -e "$f" ] || { echo "left behind: $f"; exit 1; }; done
    echo ok
' 2>&1) || fail "debian: $out"
out=$(podman run --rm --network=none -v "$DIST:/dist:ro" registry.fedoraproject.org/fedora:latest sh -euc '
    rpm -i /dist/'"$(basename "$RPM")"'
    files=$(rpm -ql crumbs | while read -r f; do [ -f "$f" ] && echo "$f"; done)
    [ -n "$files" ] || { echo "rpm -ql lists no files"; exit 1; }
    [ "$(crumbs --version)" = "crumbs '"$VERSION"'" ] || { echo "bad --version"; exit 1; }
    crumbs apps list --all --json | grep -q "\"rpm:crumbs\"" || { echo "apps list lacks rpm:crumbs"; exit 1; }
    rpm -e crumbs
    for f in $files; do [ ! -e "$f" ] || { echo "left behind: $f"; exit 1; }; done
    echo ok
' 2>&1) || fail "fedora: $out"

pass
