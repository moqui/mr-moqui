# Source this before building for wasm32-unknown-emscripten (plans/archive/Wasm.md).
#   . scripts/wasm-env.sh && cargo check -p moqui-core --no-default-features --target wasm32-unknown-emscripten
# EMSDK defaults to ~/emsdk.
: "${EMSDK:=$HOME/emsdk}"
. "$EMSDK/emsdk_env.sh" >/dev/null 2>&1 || { echo "emsdk not found at $EMSDK" >&2; return 1; }
