# scripts/release.sh: its refusals, a full build with checksums, and the
# tag workflow. The refusals run in a local clone, never the real tree.
. "$(dirname "$0")/lib.sh"
CLONE=$HOME/.cache/crumbs-test/release-clone
rm -rf "$CLONE"
git clone -q --local "$PWD" "$CLONE"
# Test the working copy of release.sh, committed in the clone so its tree is clean.
cp scripts/release.sh "$CLONE/scripts/release.sh"
git -C "$CLONE" add scripts/release.sh
git -C "$CLONE" -c user.name=test -c user.email=test@invalid commit -qm "test: release.sh" || true

check release_refuses_dirty_tree
touch "$CLONE/untracked-file"
st=0; err=$("$CLONE/scripts/release.sh" "$VERSION" 2>&1) || st=$?
[ "$st" -eq 2 ] || fail "exit $st: $err"
echo "$err" | grep -q 'refusing: the working tree has uncommitted changes' || fail "$err"
rm "$CLONE/untracked-file"

check release_refuses_version_mismatch
st=0; err=$("$CLONE/scripts/release.sh" 9.9.9 2>&1) || st=$?
[ "$st" -eq 2 ] || fail "exit $st: $err"
echo "$err" | grep -qF "version 9.9.9 does not match Cargo.toml ($VERSION)" || fail "$err"
st=0; err=$("$CLONE/scripts/release.sh" 2>&1) || st=$?
[ "$st" -eq 2 ] || fail "no version: exit $st"

check release_refuses_stale_packaging_versions
for f in packaging/aur/crum-bs/PKGBUILD packaging/aur/crum-bs-bin/.SRCINFO \
    packaging/appimage/io.github.asafelobotomy.crumbs.metainfo.xml; do
    git -C "$CLONE" checkout -q -- .
    sed -i "s/$VERSION/0.0.9/" "$CLONE/$f"
    git -C "$CLONE" -c user.name=test -c user.email=test@invalid commit -qam "test: stale $f"
    st=0; err=$("$CLONE/scripts/release.sh" "$VERSION" 2>&1) || st=$?
    [ "$st" -eq 2 ] || fail "$f at 0.0.9: exit $st: $(echo "$err" | tail -2)"
    echo "$err" | grep -qF "$f names version 0.0.9, not $VERSION" || fail "$f: $err"
    git -C "$CLONE" reset -q --hard HEAD~1
done
rm -rf "$CLONE"

check release_builds_everything_with_sums
# Run through a path with spaces: every path the script builds must stay
# one word.
SPACED="$HOME/.cache/crumbs-test/release with spaces"
rm -f "$SPACED"
ln -s "$PWD" "$SPACED"
"$SPACED/scripts/release.sh" "$VERSION" --allow-dirty >/dev/null 2>"$HOME/.cache/crumbs-test/release.log" ||
    fail "release.sh failed: $(tail -3 "$HOME/.cache/crumbs-test/release.log" | tr '\n' ' ')"
for f in "crumbs-$VERSION-x86_64-linux-musl" "crumbs-$VERSION-x86_64.AppImage" \
    "crumbs_${VERSION}_amd64.deb" "crumbs-$VERSION-1.x86_64.rpm" "crumbs-$VERSION.tar.gz" LICENSE SHA256SUMS RELEASE_NOTES.md \
    aur/crum-bs/PKGBUILD aur/crum-bs/.SRCINFO aur/crum-bs-bin/PKGBUILD aur/crum-bs-bin/.SRCINFO; do
    [ -f "dist/$f" ] || fail "missing dist/$f"
done
(cd dist && sha256sum --quiet -c SHA256SUMS) || fail "sha256sum -c failed"
[ "$(wc -l < dist/SHA256SUMS)" -eq 6 ] || fail "SHA256SUMS should list 6 files: $(cat dist/SHA256SUMS)"
grep -q "^## $VERSION\$" dist/RELEASE_NOTES.md && fail "RELEASE_NOTES.md repeats the heading"
[ -s dist/RELEASE_NOTES.md ] || fail "RELEASE_NOTES.md is empty"
for pkg in crum-bs crum-bs-bin; do
    grep -q SKIP "dist/aur/$pkg/PKGBUILD" "dist/aur/$pkg/.SRCINFO" && fail "dist/aur/$pkg still has SKIP"
    want=$(cd "dist/aur/$pkg" && makepkg --printsrcinfo) || fail "$pkg: makepkg --printsrcinfo failed"
    [ "$want" = "$(cat "dist/aur/$pkg/.SRCINFO")" ] || fail "dist/aur/$pkg/.SRCINFO does not match its PKGBUILD"
done
bin_sum=$(sha256sum "dist/crumbs-$VERSION-x86_64-linux-musl" | cut -d' ' -f1)
grep -q "$bin_sum" dist/aur/crum-bs-bin/PKGBUILD || fail "crum-bs-bin lacks the binary's sum"
# The source package downloads the tarball the release uploads, so the sum
# is of the very bytes users fetch (GitHub's own tag tarballs differ).
tar_sum=$(sha256sum "dist/crumbs-$VERSION.tar.gz" | cut -d' ' -f1)
grep -q "sha256sums=('$tar_sum')" dist/aur/crum-bs/PKGBUILD || fail "crum-bs lacks the source tarball's sum"
grep -q "source = https://github.com/asafelobotomy/crumbs/releases/download/v$VERSION/crumbs-$VERSION.tar.gz" dist/aur/crum-bs/.SRCINFO ||
    fail "crum-bs does not download the release's source tarball"
tar -tzf "dist/crumbs-$VERSION.tar.gz" | grep -qx "crumbs-$VERSION/Cargo.lock" || fail "the source tarball lacks crumbs-$VERSION/Cargo.lock"

rm -f "$SPACED"

check release_checks_licences
grep -q 'scripts/check-licenses.sh' scripts/release.sh || fail "release.sh does not run scripts/check-licenses.sh"
grep -q 'scripts/check-licenses.sh' .github/workflows/ci.yml || fail "ci.yml does not run scripts/check-licenses.sh"

check release_workflow_is_valid_yaml
wf=.github/workflows/release.yml
python3 -I - "$wf" <<'PY' || fail "release.yml is not valid"
import sys
try:
    import yaml
except ImportError:
    sys.exit(0)  # PyYAML missing: the grep checks below still run
doc = yaml.safe_load(open(sys.argv[1]))
on = doc.get("on", doc.get(True))
assert on["push"]["tags"] == ["v*"], on
assert doc["permissions"]["contents"] == "write"
PY
grep -q 'scripts/release.sh "${GITHUB_REF_NAME#v}"' "$wf" || fail "release.yml does not run scripts/release.sh"
grep -q 'gh release create' "$wf" || fail "release.yml does not run gh release create"
# Re-runnable: a release left over from a failed run is reused, its assets
# replaced, and it is published only once every upload has succeeded.
grep -q 'gh release upload .*--clobber' "$wf" || fail "release.yml does not upload with --clobber"
grep -q -- '--draft' "$wf" || fail "release.yml does not create a draft first"
for f in .github/workflows/*.yml; do
    bad=$(grep -E '^\s*(- )?uses: ' "$f" | grep -Ev '@[0-9a-f]{40}( |$)' || true)
    [ -z "$bad" ] || fail "$f: actions not pinned by commit: $bad"
done

pass
