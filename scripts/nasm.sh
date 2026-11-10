#!/usr/bin/env bash
# NASM for the Windows build of aws-lc-sys (docs/Build-Test.md). `moq` runs
# `scripts/nasm.sh ensure` before cargo and prepends the printed directory to PATH.
# The binary is not committed. Unix exits without printing anything.
#   scripts/nasm.sh ensure    print the directory that holds nasm.exe (a POSIX path; moq prepends it)
# MOQUI_NASM, a directory containing nasm.exe or the path of nasm.exe, is used as it is
# and nothing is downloaded. MOQ_DRY_RUN=1 prints the curl that would download and prints no directory.
# This may no longer be needed: AWS-LC is now built once, with aws-lc-rs's prebuilt-nasm feature on, which links the
# objects aws-lc-sys ships when nasm is not on PATH. docs/Build-Test.md (Windows) says how to check; if it holds, this
# script and moq's ensure_nasm go.
set -euo pipefail

NASM_VERSION=3.02
NASM_SHA256=161d0bfaff53c2f9e9f3e69fd0672323ebabafd1268976a5cec11be92a19aee7
NASM_URL=https://www.nasm.us/pub/nasm/releasebuilds/${NASM_VERSION}/win64/nasm-${NASM_VERSION}-win64.zip

on_windows() {
    case $(uname -s) in
        MINGW*|MSYS*|CYGWIN*) return 0 ;;
        *) return 1 ;;
    esac
}

on_windows || exit 0

[ "${1:-}" = ensure ] || { echo "usage: scripts/nasm.sh ensure" >&2; exit 1; }

to_unix() {
    if command -v cygpath >/dev/null 2>&1; then
        cygpath -u "$1"
    else
        printf '%s\n' "$1"
    fi
}

# stdout is only the directory moq prepends. Everything else goes to stderr.
# A Windows path (`C:\Users\...`) is wrong here: Git Bash turns `\U` into an escape while
# building PATH for a Windows program, so cargo.exe would not see nasm.exe. The POSIX
# path is converted to a Windows path as cargo.exe starts.
print_dir() {
    if command -v cygpath >/dev/null 2>&1; then
        cygpath -u "$1"
    else
        printf '%s\n' "$1"
    fi
}

nasm_version_ok() {
    local text
    text=$("$1" -v 2>/dev/null || true)
    case $text in
        "NASM version ${NASM_VERSION}"*) return 0 ;;
        *) return 1 ;;
    esac
}

if [ -n "${MOQUI_NASM:-}" ]; then
    target=$(to_unix "$MOQUI_NASM")
    if [ -d "$target" ]; then
        dir=$target
    elif [ -f "$target" ]; then
        dir=$(dirname "$target")
    else
        echo "MOQUI_NASM does not exist: $MOQUI_NASM" >&2
        exit 1
    fi
    if [ ! -f "$dir/nasm.exe" ] && [ ! -f "$dir/nasm" ]; then
        echo "MOQUI_NASM has no nasm: $MOQUI_NASM" >&2
        exit 1
    fi
    print_dir "$dir"
    exit 0
fi

if [ -n "${USERPROFILE:-}" ] && command -v cygpath >/dev/null 2>&1; then
    cache=$(cygpath -u "$USERPROFILE")/.moqui/nasm/$NASM_VERSION
else
    cache=${HOME}/.moqui/nasm/$NASM_VERSION
fi

if [ -f "$cache/nasm.exe" ] && nasm_version_ok "$cache/nasm.exe"; then
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
        echo "+ curl -fsSL --proxy $proxy -o nasm-${NASM_VERSION}-win64.zip $NASM_URL" >&2
    else
        echo "+ curl -fsSL -o nasm-${NASM_VERSION}-win64.zip $NASM_URL" >&2
    fi
    exit 0
fi

command -v curl >/dev/null 2>&1 || { echo "curl is required to download NASM" >&2; exit 1; }
command -v sha256sum >/dev/null 2>&1 || { echo "sha256sum is required to check NASM" >&2; exit 1; }

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
echo "downloading $NASM_URL" >&2
if [ -n "$proxy" ]; then
    curl -fsSL --proxy "$proxy" -o "$work/nasm.zip" "$NASM_URL"
else
    curl -fsSL -o "$work/nasm.zip" "$NASM_URL"
fi
actual=$(sha256sum "$work/nasm.zip" | awk '{print $1}' | tr 'A-F' 'a-f')
if [ "$actual" != "$NASM_SHA256" ]; then
    echo "SHA256 mismatch for $NASM_URL" >&2
    echo "expected $NASM_SHA256" >&2
    echo "actual   $actual" >&2
    exit 1
fi
# Git Bash tar is GNU tar and does not read zip. Windows tar does.
bsdtar=$(cygpath -u "${SYSTEMROOT:-C:\\Windows}")/System32/tar.exe
[ -x "$bsdtar" ] || { echo "Windows tar is required to unpack NASM" >&2; exit 1; }
"$bsdtar" -xf "$work/nasm.zip" -C "$work"
[ -f "$work/nasm-$NASM_VERSION/nasm.exe" ] || { echo "nasm.zip has no nasm-$NASM_VERSION/nasm.exe" >&2; exit 1; }
mkdir -p "$cache"
cp -p "$work/nasm-$NASM_VERSION/nasm.exe" "$cache/nasm.exe"
nasm_version_ok "$cache/nasm.exe" || { echo "downloaded nasm is not $NASM_VERSION" >&2; rm -f "$cache/nasm.exe"; exit 1; }
print_dir "$cache"
