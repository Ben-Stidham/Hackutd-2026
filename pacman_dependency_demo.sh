#!/usr/bin/env bash

set -euo pipefail

BASE="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"

if [[ "$EUID" -eq 0 ]]; then
    echo "Run this demo as a regular user; it uses sudo only for an isolated pacman root." >&2
    exit 1
fi

for tool in pacman makepkg repo-add sudo python3; do
    if ! command -v "$tool" >/dev/null 2>&1; then
        echo "Required Arch Linux tool not found: $tool" >&2
        exit 1
    fi
done

WORK="$(mktemp -d "${TMPDIR:-/tmp}/fixmyos-pacman-demo.XXXXXX")"
cleanup() {
    sudo rm -rf -- "$WORK"
}
trap cleanup EXIT

sudo -v

echo "Building disposable test packages and triggering a missing-dependency failure."
printf '1\nn\n' | ERROR_TEST_WORK_DIR="$WORK" bash "$BASE/generator.sh"