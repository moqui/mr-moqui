#!/usr/bin/env bash
# CMake for the Windows desktop build (cef-dll-sys, docs/Build-Test.md). `moq desktop` runs
# `scripts/cmake.sh ensure` and prepends the printed directory to PATH. The binary is not committed.
# Unix exits without printing anything.
#   scripts/cmake.sh ensure    print the directory that holds cmake.exe (a POSIX path; moq prepends it),
#                              or nothing when cmake is already on PATH
# MOQUI_CMAKE, a directory containing cmake.exe (or bin/cmake.exe) or the path of cmake.exe, is used
# as it is and nothing is downloaded. MOQ_DRY_RUN=1 prints the curl that would download and prints no directory.
set -euo pipefail

CMAKE_VERSION=4.4.4
CMAKE_SHA256=bace36e94b31c68ab6fa295f26dfa11219e0701cf7c94b0284a7d1cb13dac536
CMAKE_URL=https://github.com/Kitware/CMake/releases/download/v${CMAKE_VERSION}/cmake-${CMAKE_VERSION}-windows-x86_64.zip

on_windows() {
    case $(uname -s) in
        MINGW*|MSYS*|CYGWIN*) return 0 ;;
        *) return 1 ;;
    esac
}

on_windows || exit 0

[ "${1:-}" = ensure ] || { echo "usage: scripts/cmake.sh ensure" >&2; exit 1; }

to_unix() {
    if command -v cygpath >/dev/null 2>&1; then
        cygpath -u "$1"
    else
        printf '%s\n' "$1"
    fi
}

# stdout is only the directory moq prepends. Everything else goes to stderr.
# A Windows path (`C:\Users\...`) is wrong here: Git Bash turns `\U` into an escape while
# building PATH for a Windows program, so cargo.exe would not see cmake.exe. The POSIX
# path is converted to a Windows path as cargo.exe starts.
print_dir() {
    if command -v cygpath >/dev/null 2>&1; then
        cygpath -u "$1"
    else
        printf '%s\n' "$1"
    fi
}

# The directory that holds cmake.exe for a file, a bin directory, or an install tree. Empty if none.
cmake_bin_dir() {
    local target=$1
    if [ -d "$target" ]; then
        if [ -f "$target/cmake.exe" ] || [ -f "$target/cmake" ]; then
            printf '%s\n' "$target"
        elif [ -f "$target/bin/cmake.exe" ] || [ -f "$target/bin/cmake" ]; then
            printf '%s\n' "$target/bin"
        fi
    elif [ -f "$target" ]; then
        dirname "$target"
    fi
}

cmake_version_ok() {
    local text
    text=$("$1" --version 2>/dev/null || true)
    case $text in
        "cmake version ${CMAKE_VERSION}"*) return 0 ;;
        *) return 1 ;;
    esac
}

if [ -n "${MOQUI_CMAKE:-}" ]; then
    target=$(to_unix "$MOQUI_CMAKE")
    dir=$(cmake_bin_dir "$target")
    if [ -z "$dir" ]; then
        echo "MOQUI_CMAKE has no cmake: $MOQUI_CMAKE" >&2
        exit 1
    fi
    print_dir "$dir"
    exit 0
fi

# A cmake already on PATH is kept, whatever its version. cef-dll-sys only needs the Ninja generator.
if command -v cmake >/dev/null 2>&1; then
    exit 0
fi

if [ -n "${USERPROFILE:-}" ] && command -v cygpath >/dev/null 2>&1; then
    cache=$(cygpath -u "$USERPROFILE")/.moqui/cmake/$CMAKE_VERSION
else
    cache=${HOME}/.moqui/cmake/$CMAKE_VERSION
fi

if [ -f "$cache/bin/cmake.exe" ] && cmake_version_ok "$cache/bin/cmake.exe"; then
    print_dir "$cache/bin"
    exit 0
fi

# curl does not read git's proxy. Pass it on the command line: mingw curl does not
# always honor an https_proxy exported in this script.
proxy=${https_proxy:-${HTTPS_PROXY:-}}
if [ -z "$proxy" ] && command -v git >/dev/null 2>&1; then
    proxy=$(git config --get https.proxy || git config --get http.proxy || true)
    proxy=${proxy//$'\r'/}
fi
if [ "${MOQ_DRY_RUN:-}" = 1 ]; then
    if [ -n "$proxy" ]; then
        echo "+ curl -fsSL --proxy $proxy -o cmake-${CMAKE_VERSION}-windows-x86_64.zip $CMAKE_URL" >&2
    else
        echo "+ curl -fsSL -o cmake-${CMAKE_VERSION}-windows-x86_64.zip $CMAKE_URL" >&2
    fi
    exit 0
fi

[ "${CARGO_NET_OFFLINE:-}" != true ] || { echo "offline build needs the pinned CMake on PATH or in its cache" >&2; exit 1; }
command -v curl >/dev/null 2>&1 || { echo "curl is required to download CMake" >&2; exit 1; }
command -v sha256sum >/dev/null 2>&1 || { echo "sha256sum is required to check CMake" >&2; exit 1; }

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
echo "downloading $CMAKE_URL" >&2
if [ -n "$proxy" ]; then
    curl -fsSL --proxy "$proxy" -o "$work/cmake.zip" "$CMAKE_URL"
else
    curl -fsSL -o "$work/cmake.zip" "$CMAKE_URL"
fi
actual=$(sha256sum "$work/cmake.zip" | awk '{print $1}' | tr 'A-F' 'a-f')
if [ "$actual" != "$CMAKE_SHA256" ]; then
    echo "SHA256 mismatch for $CMAKE_URL" >&2
    echo "expected $CMAKE_SHA256" >&2
    echo "actual   $actual" >&2
    exit 1
fi
# Git Bash tar is GNU tar and does not read zip. Windows tar does.
bsdtar=$(cygpath -u "${SYSTEMROOT:-C:\\Windows}")/System32/tar.exe
[ -x "$bsdtar" ] || { echo "Windows tar is required to unpack CMake" >&2; exit 1; }
"$bsdtar" -xf "$work/cmake.zip" -C "$work"
src=$work/cmake-${CMAKE_VERSION}-windows-x86_64
[ -f "$src/bin/cmake.exe" ] || { echo "cmake.zip has no cmake-${CMAKE_VERSION}-windows-x86_64/bin/cmake.exe" >&2; exit 1; }
rm -rf "$cache"
mkdir -p "$cache"
cp -R "$src"/. "$cache"/
cmake_version_ok "$cache/bin/cmake.exe" || { echo "downloaded cmake is not $CMAKE_VERSION" >&2; rm -rf "$cache"; exit 1; }
print_dir "$cache/bin"
