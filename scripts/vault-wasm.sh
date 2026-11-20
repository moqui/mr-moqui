#!/usr/bin/env bash
# Build the vault crypto's browser module (crates/moqui-vault, feature wasm; plans/Vault-Instances.md §3.2, §3.6) and
# check it in beside its stamp:
#   scripts/vault-wasm.sh
# writes runtime/components/base/webroot/screen/webroot/js/vault/moqui_vault.wasm and moqui_vault.wasm.json. The stamp
# holds the SHA-256 over the crate's source files, its workspace dependency lines, the release profile and the
# Cargo.lock entries the module links, the list of those entries, the rustc version and the SHA-256 of the wasm.
# `cargo test -p moqui-vault` recomputes the source hash and compares the rustc version, and fails when either no
# longer matches (the_checked_in_wasm_is_what_the_source_builds); MOQUI_REGENERATE_WASM=1 on that test runs this
# script. The build is wasm32-unknown-unknown with no getrandom and no wasm-bindgen: the page gives the
# randomness. Paths are remapped so two machines on one rustc build the same bytes.
set -euo pipefail
root=$(cd "$(dirname "$0")/.." && pwd)
crate=$root/crates/moqui-vault
out=$root/runtime/components/base/webroot/screen/webroot/js/vault
cargo_home=${CARGO_HOME:-$HOME/.cargo}
# A target directory of its own: the release wasm does not churn the workspace's cache, and a run from inside a cargo
# test does not wait on that test's lock.
target_dir=$root/target/vault-wasm
features=(--no-default-features --features wasm)

if command -v sha256sum >/dev/null 2>&1; then
    sha() { sha256sum | cut -d' ' -f1; }
else
    sha() { shasum -a 256 | cut -d' ' -f1; }
fi

cd "$root"
# panic=abort: wasm32-unknown-unknown does not unwind, and the release profile's unwind is for the host.
CARGO_TARGET_DIR=$target_dir \
CARGO_PROFILE_RELEASE_PANIC=abort \
CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUSTFLAGS="--remap-path-prefix=$root=/moqui --remap-path-prefix=$cargo_home=/cargo -C strip=symbols" \
    cargo rustc -p moqui-vault --lib --release --target wasm32-unknown-unknown "${features[@]}" --crate-type cdylib --quiet
mkdir -p "$out"
cp "$target_dir/wasm32-unknown-unknown/release/moqui_vault.wasm" "$out/moqui_vault.wasm"

# The registry packages the module links, as "name version checksum" from Cargo.lock.
lock_entries() {
    cargo tree -p moqui-vault --target wasm32-unknown-unknown "${features[@]}" -e normal --prefix none \
        --format '{p}' 2>/dev/null |
        awk '!/\(/ { sub(/^v/, "", $2); print $1, $2 }' | LC_ALL=C sort -u |
        while read -r name version; do
            checksum=$(awk -v n="$name" -v v="$version" '
                $0 == "[[package]]" { name = ""; version = "" }
                $1 == "name" { gsub(/"/, "", $3); name = $3 }
                $1 == "version" { gsub(/"/, "", $3); version = $3 }
                $1 == "checksum" && name == n && version == v { gsub(/"/, "", $3); print $3 }' Cargo.lock)
            echo "$name $version $checksum"
        done
}
entries=$(lock_entries)

# The root Cargo.toml's [workspace.dependencies] line of each dependency the crate takes from the workspace.
workspace_lines() {
    grep -E '^[A-Za-z0-9_-]+(\.workspace *= *true| *= *\{ *workspace *= *true)' "$crate/Cargo.toml" |
        sed -E 's/[ .=].*//' | LC_ALL=C sort -u |
        while read -r name; do
            awk -v n="$name" '$0 == "[workspace.dependencies]" { on = 1; next } /^\[/ { on = 0 }
                on && index($0, n " = ") == 1' Cargo.toml
        done
}

# The source hash: a line "<path> <sha256>" for Cargo.toml and each src/*.rs but the tests; a line "workspace <line>"
# for each of those workspace dependency lines; a line "profile <line>" for each setting of [profile.release] in
# .cargo/config.toml; then a line "lock <name> <version> <checksum>" for each entry. The test computes the same lines.
source_sha=$(
    {
        (cd "$crate" && find Cargo.toml src -type f -name '*.rs' ! -name '*_tests.rs' -o -name Cargo.toml -type f) |
            LC_ALL=C sort -u | while read -r path; do echo "$path $(sha < "$crate/$path")"; done
        workspace_lines | while read -r line; do echo "workspace $line"; done
        awk '$0 == "[profile.release]" { on = 1; next } /^\[/ { on = 0 } on && NF && $1 !~ /^#/' .cargo/config.toml |
            while read -r line; do echo "profile $line"; done
        echo "$entries" | while read -r line; do echo "lock $line"; done
    } | sha
)
wasm_sha=$(sha < "$out/moqui_vault.wasm")
rustc_version=$(rustc -V)
{
    echo "{"
    echo "  \"source_sha256\": \"$source_sha\","
    echo "  \"rustc\": \"$rustc_version\","
    echo "  \"wasm_sha256\": \"$wasm_sha\","
    echo "  \"lock\": ["
    echo "$entries" | awk 'NR > 1 { print line "," } { line = "    \"" $0 "\"" } END { print line }'
    echo "  ]"
    echo "}"
} > "$out/moqui_vault.wasm.json"
echo "wrote $out/moqui_vault.wasm ($(wc -c < "$out/moqui_vault.wasm" | tr -d ' ') bytes) and its stamp" >&2
