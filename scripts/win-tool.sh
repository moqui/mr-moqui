# shellcheck shell=bash
# What scripts/nasm.sh, cmake.sh and ninja.sh share. Each gets one pinned Windows tool for the build, checks its
# SHA-256 and keeps it under %USERPROFILE%\.moqui; this file is sourced by them, never run.
# Their stdout is only the directory moq prepends to PATH; everything else goes to stderr.

on_windows() {
    case $(uname -s) in
        MINGW*|MSYS*|CYGWIN*) return 0 ;;
        *) return 1 ;;
    esac
}

# A POSIX path, for a path from the environment and for the directory printed for PATH. A Windows path
# (`C:\Users\...`) is wrong there: Git Bash turns `\U` into an escape while building PATH for a Windows program, so
# cargo.exe would not see the tool. The POSIX path is converted to a Windows path as cargo.exe starts.
to_unix() {
    if command -v cygpath >/dev/null 2>&1; then
        cygpath -u "$1"
    else
        printf '%s\n' "$1"
    fi
}

# The directory of the tool named by an override variable (MOQUI_NASM, ...): a directory that holds <exe>.exe or
# bin/<exe>.exe, or the path of the executable. Exits when it has none.
#   win_tool_override MOQUI_NASM nasm
win_tool_override() {
    local var=$1 exe=$2 value target
    value=${!var}
    target=$(to_unix "$value")
    if [ -f "$target" ]; then
        to_unix "$(dirname "$target")"
    elif [ -f "$target/$exe.exe" ] || [ -f "$target/$exe" ]; then
        to_unix "$target"
    elif [ -f "$target/bin/$exe.exe" ] || [ -f "$target/bin/$exe" ]; then
        to_unix "$target/bin"
    else
        echo "$var has no $exe: $value" >&2
        exit 1
    fi
}

# The cache directory of a tool's version.
#   win_tool_cache nasm 3.02
win_tool_cache() {
    if [ -n "${USERPROFILE:-}" ] && command -v cygpath >/dev/null 2>&1; then
        printf '%s\n' "$(cygpath -u "$USERPROFILE")/.moqui/$1/$2"
    else
        printf '%s\n' "$HOME/.moqui/$1/$2"
    fi
}

# Downloads a zip, checks its SHA-256 and unpacks it into a new temporary directory, WIN_TOOL_WORK, removed when the
# script exits. MOQ_DRY_RUN=1 prints the curl and exits the script, printing no directory.
#   win_tool_fetch NASM "$NASM_URL" "$NASM_SHA256" "nasm-${NASM_VERSION}-win64.zip"
win_tool_fetch() {
    local label=$1 url=$2 sha256=$3 zip=$4 proxy actual bsdtar
    # curl does not read git's proxy. Pass it on the command line: mingw curl does not always honor an https_proxy
    # exported in this script.
    proxy=${https_proxy:-${HTTPS_PROXY:-}}
    if [ -z "$proxy" ] && command -v git >/dev/null 2>&1; then
        proxy=$(git config --get https.proxy || git config --get http.proxy || true)
        proxy=${proxy//$'\r'/}
    fi
    if [ "${MOQ_DRY_RUN:-}" = 1 ]; then
        if [ -n "$proxy" ]; then
            echo "+ curl -fsSL --proxy $proxy -o $zip $url" >&2
        else
            echo "+ curl -fsSL -o $zip $url" >&2
        fi
        exit 0
    fi

    [ "${CARGO_NET_OFFLINE:-}" != true ] || { echo "offline build needs the pinned $label on PATH or in its cache" >&2; exit 1; }
    command -v curl >/dev/null 2>&1 || { echo "curl is required to download $label" >&2; exit 1; }
    command -v sha256sum >/dev/null 2>&1 || { echo "sha256sum is required to check $label" >&2; exit 1; }

    WIN_TOOL_WORK=$(mktemp -d)
    trap 'rm -rf "$WIN_TOOL_WORK"' EXIT
    echo "downloading $url" >&2
    if [ -n "$proxy" ]; then
        curl -fsSL --proxy "$proxy" -o "$WIN_TOOL_WORK/$zip" "$url"
    else
        curl -fsSL -o "$WIN_TOOL_WORK/$zip" "$url"
    fi
    actual=$(sha256sum "$WIN_TOOL_WORK/$zip" | awk '{print $1}' | tr 'A-F' 'a-f')
    if [ "$actual" != "$sha256" ]; then
        echo "SHA256 mismatch for $url" >&2
        echo "expected $sha256" >&2
        echo "actual   $actual" >&2
        exit 1
    fi
    # Git Bash tar is GNU tar and does not read zip. Windows tar does.
    bsdtar=$(cygpath -u "${SYSTEMROOT:-C:\\Windows}")/System32/tar.exe
    [ -x "$bsdtar" ] || { echo "Windows tar is required to unpack $label" >&2; exit 1; }
    "$bsdtar" -xf "$WIN_TOOL_WORK/$zip" -C "$WIN_TOOL_WORK"
}
