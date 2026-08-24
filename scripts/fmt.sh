#!/usr/bin/env bash
# The single source of truth for the formatting scope.
#
# The justfile, the pre-commit hook and CI all call this script, so the three can
# never drift apart — and none of them needs `just` to be installed.
#
# Vendored Zed crates (crates/vendor/**) are excluded on purpose: they are a
# pinned upstream snapshot carrying fmt drift that can never be made clean.
set -euo pipefail

cd "$(dirname "$0")/.."

PKGS=(
    -p mezon-app
    -p mezon-ui
    -p mezon-store
    -p mezon-client
    -p mezon-native
    -p mezon-pdf
    -p mezon-proto
    -p mezon-i18n
    -p mezon-theme
    -p mezon-widgets
    -p mezon-canvas
    -p mezon-updater
    -p mezon-audio
    -p mezon-voice
    -p mezon-stream
    -p mezon-record
    -p mmn-client
)

case "${1:-check}" in
check)
    exec cargo fmt "${PKGS[@]}" -- --check
    ;;
fix)
    exec cargo fmt "${PKGS[@]}"
    ;;
*)
    echo "usage: $0 [check|fix]" >&2
    exit 2
    ;;
esac
