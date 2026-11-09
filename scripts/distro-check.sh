#!/bin/sh
# Builds and tests crumbs inside disposable podman containers, one per distro.
#
#   scripts/distro-check.sh [image...]     run on the host (rootless podman)
#   scripts/distro-check.sh --inside       what each container runs
#
# In each container: install a toolchain, `cargo test --workspace`, read-only
# smoke tests (doctor, apps list, scan), then a real end-to-end uninstall of
# the small package `sl` as the container's root. A disposable container is
# the only place crumbs ever removes a real package in testing. The repo is
# mounted read-only; nothing from the host system is mounted.
set -eu

DEFAULT_IMAGES="docker.io/library/archlinux:latest docker.io/library/debian:trixie docker.io/library/ubuntu:latest registry.fedoraproject.org/fedora:latest"
TOOLCHAIN="1.99.1"

inside() {
    step() { printf '\n==> %s\n' "$1"; CURRENT_STEP="$1"; }
    trap 'echo "FAIL: ${CURRENT_STEP:-setup}"' EXIT

    step "install build dependencies"
    if command -v pacman >/dev/null; then
        DISTRO=arch
        pacman -Sy --noconfirm --needed gcc curl python ca-certificates >/dev/null
    elif command -v apt-get >/dev/null; then
        DISTRO=debian
        export DEBIAN_FRONTEND=noninteractive
        apt-get update -qq >/dev/null
        apt-get install -y -qq gcc libc6-dev curl python3 ca-certificates >/dev/null
    elif command -v dnf >/dev/null; then
        DISTRO=fedora
        dnf install -y -q gcc curl python3 >/dev/null
    else
        echo "unknown distro"; exit 1
    fi

    step "install rust $TOOLCHAIN"
    export PATH="$HOME/.cargo/bin:$PATH"
    if ! command -v cargo >/dev/null || ! cargo --version | grep -q "$TOOLCHAIN"; then
        curl -sSf https://sh.rustup.rs | sh -s -- -y -q --profile minimal --default-toolchain "$TOOLCHAIN" >/dev/null
    fi

    cd /src
    step "cargo test --workspace"
    cargo test -q --workspace --locked

    step "build crumbs"
    cargo build -q -p crumbs-cli --locked
    CRUMBS="$CARGO_TARGET_DIR/debug/crumbs"

    step "smoke: doctor, apps list, scan (read-only)"
    for cmd in "doctor --json" "apps list --json" "scan --json"; do
        # shellcheck disable=SC2086
        "$CRUMBS" $cmd > /tmp/out.json || { st=$?; [ "$st" -le 3 ] || exit "$st"; }
        python3 -m json.tool /tmp/out.json >/dev/null
    done

    step "end to end: uninstall sl as root"
    case "$DISTRO" in
        arch) pacman -S --noconfirm --needed sl >/dev/null; ID=pacman:sl ;;
        debian) apt-get install -y -qq sl >/dev/null; ID=dpkg:sl ;;
        fedora) dnf install -y -q sl >/dev/null; ID=rpm:sl ;;
    esac
    count() {
        case "$DISTRO" in
            arch) pacman -Qq | wc -l ;;
            debian) dpkg-query -W -f '${db:Status-Abbrev}\n' | grep -c '^ii' ;;
            fedora) rpm -qa | wc -l ;;
        esac
    }
    before=$(count)
    # As root with root's own home: CI runners set HOME to a directory root
    # does not own, and crumbs rightly refuses to act there as root.
    HOME=/root CRUMBS_ALLOW_REAL_ESCALATION=1 "$CRUMBS" uninstall "$ID" --yes --json > /tmp/uninstall.json
    python3 -m json.tool /tmp/uninstall.json >/dev/null
    case "$DISTRO" in
        arch) ! pacman -Q sl >/dev/null 2>&1 ;;
        debian) ! dpkg -s sl 2>/dev/null | grep -q '^Status: install ok installed' ;;
        fedora) ! rpm -q sl >/dev/null 2>&1 ;;
    esac
    step "end to end: exactly one package removed"
    after=$(count)
    [ "$after" -eq $((before - 1)) ] || { echo "expected $((before - 1)) packages, found $after"; exit 1; }

    trap - EXIT
    echo "PASS"
}

if [ "${1:-}" = "--inside" ]; then
    inside
    exit 0
fi

command -v podman >/dev/null || { echo "podman is required" >&2; exit 2; }
REPO=$(cd "$(dirname "$0")/.." && pwd)
IMAGES=${*:-$DEFAULT_IMAGES}
LOGDIR=${CRUMBS_DISTRO_LOGS:-$HOME/.cache/crumbs-distro-check}
mkdir -p "$LOGDIR"
failed=0
for image in $IMAGES; do
    slug=$(printf '%s' "$image" | sed 's|.*/||; s|[:.]|-|g')
    log="$LOGDIR/$slug.log"
    if podman run --rm \
        -v "$REPO:/src:ro" \
        -v "crumbs-cargo-$slug:/root/.cargo" \
        -v "crumbs-rustup-$slug:/root/.rustup" \
        -v "crumbs-target-$slug:/target" \
        -e CARGO_TARGET_DIR=/target \
        "$image" sh /src/scripts/distro-check.sh --inside >"$log" 2>&1; then
        echo "PASS $image"
    else
        echo "FAIL $image: $(grep '^FAIL: ' "$log" | tail -1 | sed 's/^FAIL: //') (log: $log)"
        failed=1
    fi
done
exit "$failed"
