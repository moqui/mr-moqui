#!/usr/bin/env bash
# Clippy, with warnings denied, for Android (aarch64-linux-android for phones, x86_64-linux-android for the emulator),
# on the client crate and the mobile app (plans/Mobile-Desktop.md §5, MD6): moqui-client, which has no Tauri, no
# framework and no async API, so the client host keeps building for the mobile app, and crates/moqui-mobile, the Tauri
# app that links it. Their TLS (rustls on aws-lc, and rustls-platform-verifier) compiles C with the NDK's clang, so
# this needs:
#   rustup target add aarch64-linux-android x86_64-linux-android
#   an Android NDK and CMake (scripts/android-env.sh finds the NDK: ANDROID_NDK_HOME, an SDK's, or Homebrew's)
#   (aws-lc-sys may need libclang for bindgen where it has no pregenerated bindings for the target)
# A run on a device needs more than this build: ./moq android (crates/moqui-mobile/README.md).
#   scripts/check-android.sh [extra clippy args...]
# ANDROID_TARGETS names the targets (default both); ANDROID_API the API level of the NDK compilers (default 26).
set -euo pipefail
cd "$(dirname "$0")/.."
root=$PWD
targets=${ANDROID_TARGETS:-aarch64-linux-android x86_64-linux-android}
for target in $targets; do
    rustup target list --installed | grep -qx "$target" \
        || { echo "the $target target is not installed: rustup target add $target" >&2; exit 1; }
done
# shellcheck source=scripts/android-env.sh
. scripts/android-env.sh
mobile=crates/moqui-mobile
# Seeded from the repository's lock file, as ./moq android does.
if [ ! -f "$mobile/Cargo.lock" ] || [ Cargo.lock -nt "$mobile/Cargo.lock" ]; then
    cp Cargo.lock "$mobile/Cargo.lock"
fi
for target in $targets; do
    echo "== cargo clippy -p moqui-client --target $target (NDK $(basename "$android_ndk"), API ${ANDROID_API:-26})" >&2
    # the client's own lints (its dependencies, moqui-lib among them, are linted on the desktops by cargo lint; they
    # are still compiled for the target here)
    cargo clippy -p moqui-client --no-deps --target "$target" --all-targets "$@" -- -D warnings
    echo "== cargo clippy in $mobile --target $target" >&2
    (cd "$mobile" && CARGO_TARGET_DIR="$root/target" cargo clippy --no-deps --target "$target" "$@" -- -D warnings)
done
