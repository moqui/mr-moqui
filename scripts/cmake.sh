#!/usr/bin/env bash
# CMake for the Windows desktop build (cef-dll-sys, docs/Build-Test.md). `moq desktop` runs
# `scripts/cmake.sh ensure` and prepends the printed directory to PATH. The binary is not committed.
# Unix exits without printing anything.
#   scripts/cmake.sh ensure    print the directory that holds cmake.exe (a POSIX path; moq prepends it),
#                              or nothing when cmake is already on PATH
# MOQUI_CMAKE, a directory containing cmake.exe (or bin/cmake.exe) or the path of cmake.exe, is used
# as it is and nothing is downloaded. MOQ_DRY_RUN=1 prints the curl that would download and prints no directory.
# The download and its check are scripts/win-tool.sh's, shared with nasm.sh and ninja.sh.
set -euo pipefail
. "$(dirname "$0")/win-tool.sh"

CMAKE_VERSION=4.4.4
CMAKE_SHA256=bace36e94b31c68ab6fa295f26dfa11219e0701cf7c94b0284a7d1cb13dac536
CMAKE_URL=https://github.com/Kitware/CMake/releases/download/v${CMAKE_VERSION}/cmake-${CMAKE_VERSION}-windows-x86_64.zip

on_windows || exit 0

[ "${1:-}" = ensure ] || { echo "usage: scripts/cmake.sh ensure" >&2; exit 1; }

cmake_version_ok() {
    local text
    text=$("$1" --version 2>/dev/null || true)
    case $text in
        "cmake version ${CMAKE_VERSION}"*) return 0 ;;
        *) return 1 ;;
    esac
}

if [ -n "${MOQUI_CMAKE:-}" ]; then
    win_tool_override MOQUI_CMAKE cmake
    exit 0
fi

# A cmake already on PATH is kept, whatever its version. cef-dll-sys only needs the Ninja generator.
if command -v cmake >/dev/null 2>&1; then
    exit 0
fi

cache=$(win_tool_cache cmake "$CMAKE_VERSION")
if [ -f "$cache/bin/cmake.exe" ] && cmake_version_ok "$cache/bin/cmake.exe"; then
    to_unix "$cache/bin"
    exit 0
fi

win_tool_fetch CMake "$CMAKE_URL" "$CMAKE_SHA256" "cmake-${CMAKE_VERSION}-windows-x86_64.zip"
src=$WIN_TOOL_WORK/cmake-${CMAKE_VERSION}-windows-x86_64
[ -f "$src/bin/cmake.exe" ] || { echo "cmake.zip has no cmake-${CMAKE_VERSION}-windows-x86_64/bin/cmake.exe" >&2; exit 1; }
cmake_version_ok "$src/bin/cmake.exe" || { echo "downloaded cmake is not $CMAKE_VERSION" >&2; exit 1; }
win_tool_install "$src" "$cache"
to_unix "$cache/bin"
