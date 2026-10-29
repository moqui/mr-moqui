#!/usr/bin/env bash
# The theseus Postgres archive that the embedded-postgres feature embeds (plans/archive/License-Review.md,
# plans/archive/EmbeddedPostgres.md, docs/Build-Test.md).
#   scripts/pg-payload.sh strip [target]    download the archive into ~/.theseus/postgresql when it is not there (sha256
#                                           checked), and repack it without bin/psql (the original stays as .orig)
#   scripts/pg-payload.sh check [archive...] fail on bin/psql, a readline library, or an ELF that NEEDs readline
#   scripts/pg-payload.sh path [target]     print where the archive is kept
# crates/moqui-host/build.rs embeds ~/.theseus/postgresql/postgresql-<version>-<target>.tar.gz (or MOQUI_PG_ARCHIVE) and
# downloads nothing; it refuses an archive that still holds psql or readline, so strip runs before the first build
# (./moq build runs it when the archive is missing). check with no arguments reads that archive and every archive
# staged under target/. Run both on every POSTGRESQL_VERSION bump.
set -euo pipefail
root=$(cd "$(dirname "$0")/.." && pwd)
version=$(sed -n 's/^POSTGRESQL_VERSION = { value = "=\([0-9.]*\)".*/\1/p' "$root/.cargo/config.toml")
[ -n "$version" ] || { echo "POSTGRESQL_VERSION is not an exact version in .cargo/config.toml" >&2; exit 1; }
host_target=$(rustc -vV | sed -n 's/^host: //p')
cache_dir=${HOME}/.theseus/postgresql
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

cached_archive() {
    echo "$cache_dir/postgresql-$version-$1.tar.gz"
}

strip_archive() {
    local target=${1:-$host_target}
    local archive name url
    archive=$(cached_archive "$target")
    name=postgresql-$version-$target
    if [ ! -f "$archive" ]; then
        url=https://github.com/theseus-rs/postgresql-binaries/releases/download/$version/$name.tar.gz
        echo "downloading $url"
        mkdir -p "$cache_dir"
        curl -fsSL -o "$work/$name.tar.gz" "$url"
        curl -fsSL -o "$work/$name.tar.gz.sha256" "$url.sha256"
        (cd "$work" && sha256sum -c "$name.tar.gz.sha256")
        mv "$work/$name.tar.gz" "$archive"
    fi
    mkdir "$work/unpack"
    tar -xzf "$archive" -C "$work/unpack"
    local removed=0
    while IFS= read -r -d '' file; do
        echo "removing ${file#"$work/unpack/"}"
        rm -f "$file"
        removed=1
    done < <(find "$work/unpack" \( -path '*/bin/psql' -o -path '*/bin/psql.exe' -o -name 'libreadline*' -o -name 'libhistory*' \) -print0)
    if [ "$removed" = 0 ]; then
        echo "$archive has no psql or readline; nothing to strip"
    else
        # Entries keep the archive's own top directory; theseus drops that first component on extract.
        (cd "$work/unpack" && tar -czf "$work/stripped.tar.gz" -- *)
        [ -f "$archive.orig" ] || cp -p "$archive" "$archive.orig"
        mv "$work/stripped.tar.gz" "$archive"
        echo "wrote $archive ($(du -h "$archive" | cut -f1))"
    fi
    check_archive "$archive"
}

# 0 when clean. Prints the NEEDED libraries of the archive's ELF files for review.
check_archive() {
    local archive=$1 dir status=0
    dir=$(mktemp -d -p "$work")
    tar -xzf "$archive" -C "$dir"
    if find "$dir" \( -path '*/bin/psql' -o -path '*/bin/psql.exe' \) | grep -q .; then
        echo "FAIL $archive: bin/psql is in the payload" >&2
        status=1
    fi
    if find "$dir" \( -name 'libreadline*' -o -name 'libhistory*' \) | grep -q .; then
        echo "FAIL $archive: a readline library is in the payload" >&2
        status=1
    fi
    local needed
    needed=$(find "$dir" -type f -print0 | while IFS= read -r -d '' file; do
        if head -c 4 "$file" | grep -q $'\x7fELF'; then
            readelf -d "$file" 2>/dev/null | sed -n "s|.*(NEEDED).*\[\(.*\)\]|${file#"$dir/"*/} \1|p"
        fi
    done)
    if printf '%s\n' "$needed" | grep -E ' lib(readline|history)\.' >&2; then
        echo "FAIL $archive: the files above NEED readline (GPL-3.0-or-later)" >&2
        status=1
    fi
    echo "$archive NEEDED:"
    printf '%s\n' "$needed" | awk '{print $2}' | sort -u | sed 's/^/  /'
    [ "$status" = 0 ] && echo "ok $archive"
    return "$status"
}

case "${1:-}" in
    strip)
        strip_archive "${2:-}"
        ;;
    check)
        shift
        archives=("$@")
        if [ "${#archives[@]}" = 0 ]; then
            [ -f "$(cached_archive "$host_target")" ] && archives+=("$(cached_archive "$host_target")")
            while IFS= read -r -d '' staged; do
                archives+=("$staged")
            done < <(find "${CARGO_TARGET_DIR:-$root/target}" -path '*/build/moqui-host-*/out/postgresql.tar.gz' -print0 2>/dev/null)
        fi
        [ "${#archives[@]}" -gt 0 ] || { echo "no archive to check" >&2; exit 1; }
        failed=0
        for archive in "${archives[@]}"; do
            check_archive "$archive" || failed=1
        done
        exit "$failed"
        ;;
    path)
        cached_archive "${2:-$host_target}"
        ;;
    *)
        sed -n '2,11p' "$0" | sed 's/^# //'
        exit 2
        ;;
esac
