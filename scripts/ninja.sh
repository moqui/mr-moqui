#!/usr/bin/env bash
# Ninja for the Windows desktop build (cef-dll-sys uses the Ninja generator, docs/Build-Test.md).
# `moq desktop` runs `scripts/ninja.sh ensure` and prepends the printed directory to PATH.
# The binary is not committed. Unix exits without printing anything.
#   scripts/ninja.sh ensure    print the directory that holds ninja.exe (a POSIX path; moq prepends it),
#                              or nothing when ninja is already on PATH
# MOQUI_NINJA, a directory containing ninja.exe or the path of ninja.exe, is used as it is and nothing
# is downloaded. A ninja already on PATH is kept. MOQ_DRY_RUN=1 prints the curl that would download
# and prints no directory.
set -euo pipefail

NINJA_VERSION=1.13.2
NINJA_SHA256=07fc8261b42b20e71d1720b39068c2e14ffcee6396b76fb7a795fb460b78dc65
NINJA_URL=https://github.com/ninja-build/ninja/releases/download/v${NINJA_VERSION}/ninja-win.zip

on_windows() {
    case $(uname -s) in
        MINGW*|MSYS*|CYGWIN*) return 0 ;;
        *) return 1 ;;
    esac
}

on_windows || exit 0

[ "${1:-}" = ensure ] || { echo "usage: scripts/ninja.sh ensure" >&2; exit 1; }

to_unix() {
    if command -v cygpath >/dev/null 2>&1; then
        cygpath -u "$1"
    else
        printf '%s\n' "$1"
    fi
}

# stdout is only the directory moq prepends. Everything else goes to stderr.
# A Windows path (`C:\Users\...`) is wrong here: Git Bash turns `\U` into an escape while
# building PATH for a Windows program, so cargo.exe would not see ninja.exe. The POSIX
# path is converted to a Windows path as cargo.exe starts.
print_dir() {
    if command -v cygpath >/dev/null 2>&1; then
        cygpath -u "$1"
    else
        printf '%s\n' "$1"
    fi
}

ninja_version_ok() {
    local text
    text=$("$1" --version 2>/dev/null || true)
    case $text in
        "$NINJA_VERSION") return 0 ;;
        *) return 1 ;;
    esac
}

if [ -n "${MOQUI_NINJA:-}" ]; then
    target=$(to_unix "$MOQUI_NINJA")
    if [ -d "$target" ]; then
        dir=$target
    elif [ -f "$target" ]; then
        dir=$(dirname "$target")
    else
        echo "MOQUI_NINJA does not exist: $MOQUI_NINJA" >&2
        exit 1
    fi
    if [ ! -f "$dir/ninja.exe" ] && [ ! -f "$dir/ninja" ]; then
        echo "MOQUI_NINJA has no ninja: $MOQUI_NINJA" >&2
        exit 1
    fi
    print_dir "$dir"
    exit 0
fi

# A ninja already on PATH is kept. The generator only needs the ninja command.
if command -v ninja >/dev/null 2>&1; then
    exit 0
fi

if [ -n "${USERPROFILE:-}" ] && command -v cygpath >/dev/null 2>&1; then
    cache=$(cygpath -u "$USERPROFILE")/.moqui/ninja/$NINJA_VERSION
else
    cache=${HOME}/.moqui/ninja/$NINJA_VERSION
fi

if [ -f "$cache/ninja.exe" ] && ninja_version_ok "$cache/ninja.exe"; then
    print_dir "$cache"
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
        echo "+ curl -fsSL --proxy $proxy -o ninja-win.zip $NINJA_URL" >&2
    else
        echo "+ curl -fsSL -o ninja-win.zip $NINJA_URL" >&2
    fi
    exit 0
fi

[ "${CARGO_NET_OFFLINE:-}" != true ] || { echo "offline build needs the pinned Ninja on PATH or in its cache" >&2; exit 1; }
command -v curl >/dev/null 2>&1 || { echo "curl is required to download Ninja" >&2; exit 1; }
command -v sha256sum >/dev/null 2>&1 || { echo "sha256sum is required to check Ninja" >&2; exit 1; }

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
echo "downloading $NINJA_URL" >&2
if [ -n "$proxy" ]; then
    curl -fsSL --proxy "$proxy" -o "$work/ninja.zip" "$NINJA_URL"
else
    curl -fsSL -o "$work/ninja.zip" "$NINJA_URL"
fi
actual=$(sha256sum "$work/ninja.zip" | awk '{print $1}' | tr 'A-F' 'a-f')
if [ "$actual" != "$NINJA_SHA256" ]; then
    echo "SHA256 mismatch for $NINJA_URL" >&2
    echo "expected $NINJA_SHA256" >&2
    echo "actual   $actual" >&2
    exit 1
fi
# Git Bash tar is GNU tar and does not read zip. Windows tar does.
bsdtar=$(cygpath -u "${SYSTEMROOT:-C:\\Windows}")/System32/tar.exe
[ -x "$bsdtar" ] || { echo "Windows tar is required to unpack Ninja" >&2; exit 1; }
"$bsdtar" -xf "$work/ninja.zip" -C "$work"
[ -f "$work/ninja.exe" ] || { echo "ninja.zip has no ninja.exe" >&2; exit 1; }
mkdir -p "$cache"
cp -p "$work/ninja.exe" "$cache/ninja.exe"
ninja_version_ok "$cache/ninja.exe" || { echo "downloaded ninja is not $NINJA_VERSION" >&2; rm -f "$cache/ninja.exe"; exit 1; }
print_dir "$cache"
