#!/usr/bin/env bash
# Clippy, with warnings denied, on the client crate for Android (aarch64-linux-android), so the client host of the
# desktop app's client mode keeps building for the mobile app (plans/Mobile-Desktop.md §5, MD6). Only moqui-client: it
# has no Tauri, no framework and no async API. Its TLS (rustls on aws-lc, and rustls-platform-verifier) compiles C with
# the NDK's clang, so this needs:
#   rustup target add aarch64-linux-android
#   an Android NDK, in ANDROID_NDK_HOME (else the newest under $ANDROID_HOME/ndk or ~/Library/Android/sdk/ndk), and CMake
#   (aws-lc-sys may need libclang for bindgen where it has no pregenerated bindings for the target)
# A run on an app needs more than this build: rustls-platform-verifier's Android component (crates/moqui-client/README.md).
#   scripts/check-android.sh [extra clippy args...]
set -euo pipefail
cd "$(dirname "$0")/.."
target=aarch64-linux-android
api=${ANDROID_API:-26}
rustup target list --installed | grep -qx "$target" \
    || { echo "the $target target is not installed: rustup target add $target" >&2; exit 1; }
ndk=${ANDROID_NDK_HOME:-${ANDROID_NDK_ROOT:-}}
if [ -z "$ndk" ]; then
    for sdk in "${ANDROID_HOME:-}" "$HOME/Library/Android/sdk" "$HOME/Android/Sdk"; do
        [ -n "$sdk" ] && [ -d "$sdk/ndk" ] || continue
        ndk=$(find "$sdk/ndk" -mindepth 1 -maxdepth 1 -type d | sort -V | tail -1)
        [ -n "$ndk" ] && break
    done
fi
[ -n "$ndk" ] && [ -d "$ndk" ] || { echo "no Android NDK: set ANDROID_NDK_HOME" >&2; exit 1; }
host=$(uname -s | tr '[:upper:]' '[:lower:]')-x86_64
bin="$ndk/toolchains/llvm/prebuilt/$host/bin"
[ -d "$bin" ] || { echo "no NDK toolchain at $bin" >&2; exit 1; }
export ANDROID_NDK_HOME="$ndk"
export CC_aarch64_linux_android="$bin/${target}${api}-clang"
export AR_aarch64_linux_android="$bin/llvm-ar"
export CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER="$CC_aarch64_linux_android"
echo "== cargo clippy -p moqui-client --target $target (NDK $(basename "$ndk"), API $api)" >&2
cargo clippy -p moqui-client --target "$target" --all-targets "$@" -- -D warnings
