#!/bin/sh
# Runs the packaging tests: scripts/tests/<name>.sh for each name given, or
# all of them. Each test file runs its checks in order and stops at the
# first failure; this prints `PASS <name>` or `FAIL <name>: <reason>` and
# exits non-zero if any failed. Build the artifacts first (scripts/build-*.sh).
set -u
cd "$(dirname "$0")/.."
if [ $# -eq 0 ]; then
    set -- $(for f in scripts/tests/*.sh; do b=$(basename "$f" .sh); [ "$b" = lib ] || echo "$b"; done)
fi
LOGDIR=${CRUMBS_PACKAGING_LOGS:-$HOME/.cache/crumbs-packaging-tests}
mkdir -p "$LOGDIR"
failed=0
for name in "$@"; do
    log="$LOGDIR/$name.log"
    if sh "scripts/tests/$name.sh" >"$log" 2>&1; then
        echo "PASS $name"
    else
        reason=$(grep '^FAIL: ' "$log" | tail -1 | sed 's/^FAIL: //')
        echo "FAIL $name: ${reason:-see $log} (log: $log)"
        failed=1
    fi
done
exit "$failed"
