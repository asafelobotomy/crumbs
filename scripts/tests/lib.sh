# Shared helpers for scripts/tests/*.sh. Sourced, not run.
set -eu
cd "$(dirname "$0")/../.."
VERSION=$(cargo metadata --format-version 1 --no-deps --locked |
    python3 -c 'import json,sys; print(next(p["version"] for p in json.load(sys.stdin)["packages"] if p["name"] == "crumbs-cli"))')
DIST=$PWD/dist
STATIC=$DIST/crumbs-$VERSION-x86_64-linux-musl
IMAGES="docker.io/library/archlinux:latest docker.io/library/debian:trixie docker.io/library/ubuntu:latest registry.fedoraproject.org/fedora:latest"

CURRENT=setup
trap 'echo "FAIL: $CURRENT${WHY:+: $WHY}"' EXIT

# `check <name>`: starts the named check; its failure is reported by name.
check() { CURRENT=$1; WHY=; echo "==> $1"; }
# `fail <why>`: fails the current check.
fail() { WHY=$1; exit 1; }
# `need <file>`: fails unless the artifact exists.
need() { [ -e "$1" ] || fail "missing $1 (build it first)"; }
# `pass`: ends the file successfully.
pass() { trap - EXIT; echo PASS; }
