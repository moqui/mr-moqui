#!/usr/bin/env bash
# Build the vault crypto's browser module (crates/moqui-vault, feature wasm; plans/Vault-Instances.md §3.2, §3.6) on
# demand. It is not kept in git: `./moq build` (every package and --embed level), `./moq archive`, podman/build.sh and
# the tasks that run moqui (test, test-client, test-framework, test-apps, test-all, accept, run, ...) run this first.
#   scripts/vault-wasm.sh            build it when it is missing or stale, else say it is up to date
#   scripts/vault-wasm.sh --quiet    the same, with nothing said when it is up to date (what moq runs)
#   scripts/vault-wasm.sh --check    exit 1 when it is missing or stale (and say why), 0 when up to date; builds nothing
#   scripts/vault-wasm.sh --force    build it whatever the stamp says
# writes runtime/components/base/webroot/screen/webroot/js/vault/moqui_vault.wasm and beside it its stamp,
# moqui_vault.wasm.json, the build record (both ignored by git). The stamp holds the SHA-256 over the crate's source
# files, its workspace dependency lines, the release profile and the Cargo.lock entries the module links, the list of
# those entries, the rustc version and the SHA-256 of the wasm; the module is rebuilt when any of those changed, or the
# wasm is not the one the stamp names. The build is wasm32-unknown-unknown with no getrandom and no wasm-bindgen (the
# page gives the randomness), so it refuses a dependency tree that has either. The target is added with rustup when it
# is missing (for the toolchain rust-toolchain.toml pins); without rustup it says what to install. Paths are remapped
# so two machines on one rustc build the same bytes.
set -euo pipefail
root=$(cd "$(dirname "$0")/.." && pwd)
crate=$root/crates/moqui-vault
out=$root/runtime/components/base/webroot/screen/webroot/js/vault
wasm=$out/moqui_vault.wasm
stamp=$out/moqui_vault.wasm.json
target=wasm32-unknown-unknown
cargo_home=${CARGO_HOME:-$HOME/.cargo}
# A target directory of its own: the release wasm does not churn the workspace's cache, and a run from moq does not
# wait on a workspace build's lock.
target_dir=$root/target/vault-wasm
features=(--no-default-features --features wasm)

mode=build
quiet=
for arg in "$@"; do
    case $arg in
        --check) mode=check ;;
        --force) mode=force ;;
        --quiet | -q) quiet=1 ;;
        -h | --help)
            sed -n '2,/^set -euo/{/^set -euo/d;s/^# \{0,1\}//;p;}' "$0"
            exit 0
            ;;
        *)
            echo "vault-wasm.sh: unknown argument $arg (--check, --force, --quiet)" >&2
            exit 2
            ;;
    esac
done

die() {
    echo "vault-wasm.sh: $*" >&2
    exit 1
}

if command -v sha256sum >/dev/null 2>&1; then
    sha() { sha256sum | cut -d' ' -f1; }
else
    sha() { shasum -a 256 | cut -d' ' -f1; }
fi

cd "$root"

# Every registry package of Cargo.lock, as "name version checksum".
lock_all() {
    awk '$0 == "[[package]]" { name = ""; version = "" }
        $1 == "name" { gsub(/"/, "", $3); name = $3 }
        $1 == "version" { gsub(/"/, "", $3); version = $3 }
        $1 == "checksum" { gsub(/"/, "", $3); print name, version, $3 }' Cargo.lock
}

# The registry packages the module links, as "name version checksum" from Cargo.lock (cargo tree: a build only).
lock_entries() {
    local all tree
    all=$(lock_all)
    tree=$(cargo tree -p moqui-vault --target "$target" "${features[@]}" -e normal --prefix none --format '{p}') ||
        die "cargo tree failed for the module's dependencies"
    # Here-strings, not pipes, where the reader may stop early: under pipefail the writer's SIGPIPE would fail.
    if grep -Eq '^(getrandom|wasm-bindgen) ' <<<"$tree"; then
        die "the module's dependency tree has $(grep -E '^(getrandom|wasm-bindgen) ' <<<"$tree" |
            LC_ALL=C sort -u | paste -sd, -): it takes its randomness from the page (crates/moqui-vault/Cargo.toml)"
    fi
    printf '%s\n' "$tree" | awk '!/\(/ { sub(/^v/, "", $2); print $1, $2 }' | LC_ALL=C sort -u |
        while read -r name version; do
            awk -v n="$name" -v v="$version" '$1 == n && $2 == v { print; exit }' <<<"$all"
        done
}

# The root Cargo.toml's [workspace.dependencies] line of each dependency the crate takes from the workspace.
workspace_lines() {
    grep -E '^[A-Za-z0-9_-]+(\.workspace *= *true| *= *\{ *workspace *= *true)' "$crate/Cargo.toml" |
        sed -E 's/[ .=].*//' | LC_ALL=C sort -u |
        while read -r name; do
            awk -v n="$name" '$0 == "[workspace.dependencies]" { on = 1; next } /^\[/ { on = 0 }
                on && index($0, n " = ") == 1' Cargo.toml
        done
}

# The source hash over the lock entries in $1: a line "<path> <sha256>" for Cargo.toml and each src/*.rs but the
# tests; a line "workspace <line>" for each of those workspace dependency lines; a line "profile <line>" for each
# setting of [profile.release] in .cargo/config.toml; then a line "lock <name> <version> <checksum>" for each entry.
source_sha() {
    {
        (cd "$crate" && find Cargo.toml src -type f -name '*.rs' ! -name '*_tests.rs' -o -name Cargo.toml -type f) |
            LC_ALL=C sort -u | while read -r path; do echo "$path $(sha <"$crate/$path")"; done
        workspace_lines | while read -r line; do echo "workspace $line"; done
        awk '$0 == "[profile.release]" { on = 1; next } /^\[/ { on = 0 } on && NF && $1 !~ /^#/' .cargo/config.toml |
            while read -r line; do echo "profile $line"; done
        printf '%s\n' "$1" | while read -r line; do echo "lock $line"; done
    } | sha
}

# A string field of the stamp.
stamp_field() {
    sed -n 's/^ *"'"$1"'": *"\(.*\)",\{0,1\}$/\1/p' "$stamp" | head -n 1
}

# Why the module must be built, or nothing when it is up to date. Needs rustc and no cargo, so it is cheap and
# offline: the lock entries are the stamp's, each checked against Cargo.lock (a changed dependency changes one of them,
# or the crate's or the workspace's manifest).
stale_reason() {
    [ -f "$wasm" ] || { echo "it is not built"; return; }
    [ -f "$stamp" ] || { echo "it has no stamp"; return; }
    local entries changed rustc_version
    entries=$(sed -n '/"lock": \[/,/\]/{s/^ *"\([^"]*\)",\{0,1\}$/\1/p;}' "$stamp")
    # The stamp's entries Cargo.lock no longer has, the first named.
    changed=$(grep -vxF -f <(lock_all) <<<"$entries" | head -n 1) || true
    [ -z "$changed" ] || { echo "Cargo.lock changed ${changed% *}"; return; }
    [ "$(stamp_field source_sha256)" = "$(source_sha "$entries")" ] ||
        { echo "crates/moqui-vault, its workspace dependency lines or the release profile changed"; return; }
    rustc_version=$(rustc -V 2>/dev/null) || { echo "rustc -V did not run"; return; }
    [ "$(stamp_field rustc)" = "$rustc_version" ] || { echo "it was built by another rustc ($(stamp_field rustc))"; return; }
    [ "$(stamp_field wasm_sha256)" = "$(sha <"$wasm")" ] || { echo "the module is not the one its stamp names"; return; }
}

# The wasm32 target for the rustc that builds here (the toolchain rust-toolchain.toml pins, unless RUSTUP_TOOLCHAIN
# names another): added with rustup when it is missing.
need_target() {
    command -v cargo >/dev/null 2>&1 || die "cargo is not on the PATH: install Rust (https://rustup.rs), then run this again"
    local sysroot
    sysroot=$(rustc --print sysroot) || die "rustc --print sysroot failed"
    [ ! -d "$sysroot/lib/rustlib/$target" ] || return 0
    [ "${CARGO_NET_OFFLINE:-}" != true ] || die "offline build needs the preinstalled $target Rust target"
    if command -v rustup >/dev/null 2>&1; then
        echo "vault-wasm.sh: adding the $target target to $(rustc -V) (rustup target add $target)" >&2
        rustup target add "$target" >&2 || die "rustup target add $target failed"
        return 0
    fi
    die "the $target target is not installed for $(rustc -V), and there is no rustup to add it: rustup target add" \
        "$target, or install that target of your Rust distribution"
}

if [ "$mode" != force ]; then
    reason=$(stale_reason)
    if [ -z "$reason" ]; then
        [ -n "$quiet" ] || echo "vault-wasm.sh: ${wasm#"$root"/} is up to date" >&2
        exit 0
    fi
    if [ "$mode" = check ]; then
        echo "vault-wasm.sh: ${wasm#"$root"/} is stale: $reason (scripts/vault-wasm.sh, or ./moq build, builds it)" >&2
        exit 1
    fi
    echo "vault-wasm.sh: building the vault's browser module: $reason" >&2
fi

need_target
entries=$(lock_entries) || exit 1
# panic=abort: wasm32-unknown-unknown does not unwind, and the release profile's unwind is for the host.
CARGO_TARGET_DIR=$target_dir \
CARGO_PROFILE_RELEASE_PANIC=abort \
CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUSTFLAGS="--remap-path-prefix=$root=/moqui --remap-path-prefix=$cargo_home=/cargo -C strip=symbols" \
    cargo rustc -p moqui-vault --lib --release --target "$target" "${features[@]}" --crate-type cdylib --quiet
mkdir -p "$out"
# Written aside and moved, so a server reading the module never gets half of one, and a failed build leaves no stamp
# that names it.
rm -f "$stamp"
cp "$target_dir/$target/release/moqui_vault.wasm" "$wasm.part"
mv -f "$wasm.part" "$wasm"

{
    echo "{"
    echo "  \"source_sha256\": \"$(source_sha "$entries")\","
    echo "  \"rustc\": \"$(rustc -V)\","
    echo "  \"wasm_sha256\": \"$(sha <"$wasm")\","
    echo "  \"lock\": ["
    printf '%s\n' "$entries" | awk 'NR > 1 { print line "," } { line = "    \"" $0 "\"" } END { print line }'
    echo "  ]"
    echo "}"
} >"$stamp.part"
mv -f "$stamp.part" "$stamp"
echo "vault-wasm.sh: wrote ${wasm#"$root"/} ($(wc -c <"$wasm" | tr -d ' ') bytes) and its stamp" >&2
