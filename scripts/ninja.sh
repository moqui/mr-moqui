#!/usr/bin/env bash
# Ninja for the Windows desktop build (cef-dll-sys uses the Ninja generator, docs/Build-Test.md).
# `moq desktop` runs `scripts/ninja.sh ensure` and prepends the printed directory to PATH.
# The binary is not committed. Unix exits without printing anything.
#   scripts/ninja.sh ensure    print the directory that holds ninja.exe (a POSIX path; moq prepends it),
#                              or nothing when ninja is already on PATH
# MOQUI_NINJA, a directory containing ninja.exe or the path of ninja.exe, is used as it is and nothing
# is downloaded. A ninja already on PATH is kept. MOQ_DRY_RUN=1 prints the curl that would download
# and prints no directory. The download and its check are scripts/win-tool.sh's, shared with nasm.sh and cmake.sh.
set -euo pipefail
. "$(dirname "$0")/win-tool.sh"

NINJA_VERSION=1.13.2
NINJA_SHA256=07fc8261b42b20e71d1720b39068c2e14ffcee6396b76fb7a795fb460b78dc65
NINJA_URL=https://github.com/ninja-build/ninja/releases/download/v${NINJA_VERSION}/ninja-win.zip

on_windows || exit 0

[ "${1:-}" = ensure ] || { echo "usage: scripts/ninja.sh ensure" >&2; exit 1; }

ninja_version_ok() {
    local text
    text=$("$1" --version 2>/dev/null || true)
    case $text in
        "$NINJA_VERSION") return 0 ;;
        *) return 1 ;;
    esac
}

if [ -n "${MOQUI_NINJA:-}" ]; then
    win_tool_override MOQUI_NINJA ninja
    exit 0
fi

# A ninja already on PATH is kept. The generator only needs the ninja command.
if command -v ninja >/dev/null 2>&1; then
    exit 0
fi

cache=$(win_tool_cache ninja "$NINJA_VERSION")
if [ -f "$cache/ninja.exe" ] && ninja_version_ok "$cache/ninja.exe"; then
    to_unix "$cache"
    exit 0
fi

win_tool_fetch Ninja "$NINJA_URL" "$NINJA_SHA256" ninja-win.zip
[ -f "$WIN_TOOL_WORK/ninja.exe" ] || { echo "ninja.zip has no ninja.exe" >&2; exit 1; }
ninja_version_ok "$WIN_TOOL_WORK/ninja.exe" || { echo "downloaded ninja is not $NINJA_VERSION" >&2; exit 1; }
win_tool_install "$WIN_TOOL_WORK/ninja.exe" "$cache/ninja.exe"
to_unix "$cache"
