# The static musl binary: linkage, runs on every distro, release guards.
. "$(dirname "$0")/lib.sh"

check static_binary_is_statically_linked
need "$STATIC"
file "$STATIC" | grep -Eq 'statically linked|static-pie linked' || fail "file: $(file -b "$STATIC")"
ldd_out=$(ldd "$STATIC" 2>&1 || true)
echo "$ldd_out" | grep -Eq 'not a dynamic executable|statically linked' || fail "ldd: $ldd_out"

check static_binary_runs_in_every_distro
for image in $IMAGES; do
    out=$(podman run --rm --network=none -v "$DIST:/dist:ro" "$image" "/dist/$(basename "$STATIC")" --version) ||
        fail "$image: exit $?"
    [ "$out" = "crumbs $VERSION" ] || fail "$image printed: $out"
done

check static_binary_keeps_release_guards
st=0; err=$(CRUMBS_ROOT=/nonexistent "$STATIC" scan 2>&1 >/dev/null) || st=$?
[ "$st" -eq 2 ] || fail "CRUMBS_ROOT: exit $st"
echo "$err" | grep -q 'honoured only by debug builds' || fail "CRUMBS_ROOT: $err"
st=0; CRUMBS_TEST_PRIV_RUNNER=sh "$STATIC" doctor >/dev/null 2>&1 || st=$?
[ "$st" -eq 2 ] || fail "CRUMBS_TEST_PRIV_RUNNER: exit $st"

pass
