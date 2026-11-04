# Source this before an Android build (scripts/check-android.sh, ./moq android): finds the SDK, the NDK and a JDK, and
# exports what cargo and the Tauri CLI read.
#   . scripts/android-env.sh || exit 1
# - ANDROID_HOME: ANDROID_HOME, else ANDROID_SDK_ROOT, else ~/Android/Sdk or ~/Library/Android/sdk. Optional for a
#   clippy run (only the NDK is needed), required by the Tauri CLI.
# - ANDROID_NDK_HOME and NDK_HOME: ANDROID_NDK_HOME, else ANDROID_NDK_ROOT, else NDK_HOME, else the newest under the
#   SDK's ndk/, else Homebrew's (brew install --cask android-ndk).
# - JAVA_HOME: JAVA_HOME, else the JDK of the javac on the PATH (Gradle needs a JDK, 17 or later).
# - CC_<target>, AR_<target> and CARGO_TARGET_<TARGET>_LINKER for aarch64-linux-android and x86_64-linux-android at
#   API ANDROID_API (default 26), for the crates that compile C (aws-lc, SQLite).
# Sets android_sdk, android_ndk and android_ndk_bin for the caller. Returns 1, with a message, without an NDK.
android_sdk=${ANDROID_HOME:-${ANDROID_SDK_ROOT:-}}
if [ -z "$android_sdk" ]; then
    for _sdk in "$HOME/Android/Sdk" "$HOME/Library/Android/sdk"; do
        [ -d "$_sdk" ] && { android_sdk=$_sdk; break; }
    done
fi
[ -z "$android_sdk" ] || export ANDROID_HOME="$android_sdk"
android_ndk=${ANDROID_NDK_HOME:-${ANDROID_NDK_ROOT:-${NDK_HOME:-}}}
if [ -z "$android_ndk" ] && [ -n "$android_sdk" ] && [ -d "$android_sdk/ndk" ]; then
    android_ndk=$(find "$android_sdk/ndk" -mindepth 1 -maxdepth 1 -type d | sort -V | tail -1)
fi
if [ -z "$android_ndk" ]; then
    for _ndk in /opt/homebrew/share/android-ndk /usr/local/share/android-ndk; do
        [ -d "$_ndk" ] && { android_ndk=$_ndk; break; }
    done
fi
unset _sdk _ndk
if [ -z "$android_ndk" ] || [ ! -d "$android_ndk" ]; then
    echo "no Android NDK: set ANDROID_NDK_HOME, or install one with sdkmanager 'ndk;<version>'" >&2
    return 1
fi
export ANDROID_NDK_HOME="$android_ndk" NDK_HOME="$android_ndk"
android_ndk_bin="$android_ndk/toolchains/llvm/prebuilt/$(uname -s | tr '[:upper:]' '[:lower:]')-x86_64/bin"
if [ ! -d "$android_ndk_bin" ]; then
    echo "no NDK toolchain at $android_ndk_bin" >&2
    return 1
fi
export CC_aarch64_linux_android="$android_ndk_bin/aarch64-linux-android${ANDROID_API:-26}-clang"
export AR_aarch64_linux_android="$android_ndk_bin/llvm-ar"
export CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER="$CC_aarch64_linux_android"
export CC_x86_64_linux_android="$android_ndk_bin/x86_64-linux-android${ANDROID_API:-26}-clang"
export AR_x86_64_linux_android="$android_ndk_bin/llvm-ar"
export CARGO_TARGET_X86_64_LINUX_ANDROID_LINKER="$CC_x86_64_linux_android"
if [ -z "${JAVA_HOME:-}" ] && command -v javac >/dev/null 2>&1; then
    JAVA_HOME=$(dirname "$(dirname "$(readlink -f "$(command -v javac)")")")
    export JAVA_HOME
fi
