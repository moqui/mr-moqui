#!/usr/bin/env bash
# Build the browser module and stage it with the JS wrappers (plans/archive/Wasm.md).
#   scripts/wasm-build.sh [--release] [out-dir]
# out-dir defaults to <target>/wasm-pkg/<profile>. It holds moqui_browser.mjs, .wasm, .data,
# and js/*.js; serve that directory, or run scripts/wasm-smoke.mjs against it.
# The Chromium smokes are `cargo test -p moqui-host --test it -- wasm_browser`. Build with
# CARGO_TARGET_DIR=target/browser so the package is target/browser/wasm-pkg/debug, or set MOQUI_WASM_PKG.
set -euo pipefail
root=$(cd "$(dirname "$0")/.." && pwd)
profile=debug
flag=()
if [ "${1:-}" = "--release" ]; then
    profile=release
    flag=(--release)
    shift
fi
target_dir=${CARGO_TARGET_DIR:-$root/target}
case "$target_dir" in /*) ;; *) target_dir=$root/$target_dir ;; esac
out=${1:-$target_dir/wasm-pkg/$profile}
# emsdk_env.sh is not written for set -u.
set +u
. "$root/scripts/wasm-env.sh"
set -u
(cd "$root" && cargo build -p moqui-browser --target wasm32-unknown-emscripten "${flag[@]}")
# cargo copies the .js and .wasm up a level but not the preload .data, so take all three from deps.
deps=$target_dir/wasm32-unknown-emscripten/$profile/deps
mkdir -p "$out"
cp "$deps/moqui_browser.wasm" "$deps/moqui_browser.data" "$out/"
# EXPORT_ES6 output; .mjs so Node loads it as a module without a package.json.
cp "$deps/moqui_browser.js" "$out/moqui_browser.mjs"
cp "$root"/crates/moqui-browser/js/*.js "$root"/crates/moqui-browser/js/*.html "$out/"
echo "$out"
