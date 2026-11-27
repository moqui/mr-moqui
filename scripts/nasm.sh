#!/usr/bin/env bash
# NASM for the Windows build of aws-lc-sys (docs/Build-Test.md). `moq` runs
# `scripts/nasm.sh ensure` before cargo and prepends the printed directory to PATH.
# The binary is not committed. Unix exits without printing anything.
#   scripts/nasm.sh ensure    print the directory that holds nasm.exe (a POSIX path; moq prepends it)
# MOQUI_NASM, a directory containing nasm.exe or the path of nasm.exe, is used as it is
# and nothing is downloaded. MOQ_DRY_RUN=1 prints the curl that would download and prints no directory.
# The download and its check are scripts/win-tool.sh's, shared with cmake.sh and ninja.sh.
# This may no longer be needed: AWS-LC is now built once, with aws-lc-rs's prebuilt-nasm feature on, which links the
# objects aws-lc-sys ships when nasm is not on PATH. docs/Build-Test.md (Windows) says how to check; if it holds, this
# script and moq's ensure_nasm go.
set -euo pipefail
. "$(dirname "$0")/win-tool.sh"

NASM_VERSION=3.02
NASM_SHA256=161d0bfaff53c2f9e9f3e69fd0672323ebabafd1268976a5cec11be92a19aee7
NASM_URL=https://www.nasm.us/pub/nasm/releasebuilds/${NASM_VERSION}/win64/nasm-${NASM_VERSION}-win64.zip

on_windows || exit 0

[ "${1:-}" = ensure ] || { echo "usage: scripts/nasm.sh ensure" >&2; exit 1; }

nasm_version_ok() {
    local text
    text=$("$1" -v 2>/dev/null || true)
    case $text in
        "NASM version ${NASM_VERSION}"*) return 0 ;;
        *) return 1 ;;
    esac
}

if [ -n "${MOQUI_NASM:-}" ]; then
    win_tool_override MOQUI_NASM nasm
    exit 0
fi

cache=$(win_tool_cache nasm "$NASM_VERSION")
if [ -f "$cache/nasm.exe" ] && nasm_version_ok "$cache/nasm.exe"; then
    to_unix "$cache"
    exit 0
fi

win_tool_fetch NASM "$NASM_URL" "$NASM_SHA256" "nasm-${NASM_VERSION}-win64.zip"
src=$WIN_TOOL_WORK/nasm-$NASM_VERSION/nasm.exe
[ -f "$src" ] || { echo "nasm.zip has no nasm-$NASM_VERSION/nasm.exe" >&2; exit 1; }
mkdir -p "$cache"
cp -p "$src" "$cache/nasm.exe"
nasm_version_ok "$cache/nasm.exe" || { echo "downloaded nasm is not $NASM_VERSION" >&2; rm -f "$cache/nasm.exe"; exit 1; }
to_unix "$cache"
