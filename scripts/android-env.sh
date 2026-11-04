# Source this before an Android build (scripts/check-android.sh, ./moq android): finds the SDK, the NDK and a JDK, and
# exports what cargo and the Tauri CLI read.
#   . scripts/android-env.sh || exit 1
# - ANDROID_HOME: ANDROID_HOME, else ANDROID_SDK_ROOT, else ~/Android/Sdk or ~/Library/Android/sdk, the first that is
#   a directory. Optional for a clippy run (only the NDK is needed), required by the Tauri CLI and the emulator.
# - ANDROID_NDK_HOME and NDK_HOME: ANDROID_NDK_HOME, else ANDROID_NDK_ROOT, else NDK_HOME, else the newest under the ndk/
#   of the first of those SDK locations that has one, else Homebrew's (brew install --cask android-ndk). With
#   ANDROID_ENV_NO_NDK=1 (the emulator) none is needed.
# - JAVA_HOME: JAVA_HOME, else on macOS /usr/libexec/java_home, then Android Studio's bundled JBR, elsewhere the JDK of
#   the javac on the PATH; only a directory with an executable bin/javac (Gradle needs a JDK, 17 or later).
# - CC_<target>, AR_<target> and CARGO_TARGET_<TARGET>_LINKER for aarch64-linux-android and x86_64-linux-android at
#   API ANDROID_API (default 26), for the crates that compile C (aws-lc, SQLite).
# Sets android_sdk, android_ndk and android_ndk_bin for the caller. Returns 1, with a message, without an NDK (unless
# ANDROID_ENV_NO_NDK=1).
android_sdk=
android_ndk=${ANDROID_NDK_HOME:-${ANDROID_NDK_ROOT:-${NDK_HOME:-}}}
for _sdk in "${ANDROID_HOME:-}" "${ANDROID_SDK_ROOT:-}" "$HOME/Android/Sdk" "$HOME/Library/Android/sdk"; do
    [ -n "$_sdk" ] && [ -d "$_sdk" ] || continue
    [ -n "$android_sdk" ] || android_sdk=$_sdk
    if [ -z "$android_ndk" ] && [ -d "$_sdk/ndk" ]; then
        android_ndk=$(find "$_sdk/ndk" -mindepth 1 -maxdepth 1 -type d | sort -V | tail -1)
    fi
done
[ -z "$android_sdk" ] || export ANDROID_HOME="$android_sdk"
if [ -z "$android_ndk" ]; then
    for _ndk in /opt/homebrew/share/android-ndk /usr/local/share/android-ndk; do
        [ -d "$_ndk" ] && { android_ndk=$_ndk; break; }
    done
fi
unset _sdk _ndk

# The JDK.
if [ -z "${JAVA_HOME:-}" ]; then
    _jdks=()
    if [ "$(uname -s)" = Darwin ]; then
        _jdks+=("$(/usr/libexec/java_home 2>/dev/null || true)")
        _jdks+=("$HOME/Applications/Android Studio.app/Contents/jbr/Contents/Home")
        _jdks+=("/Applications/Android Studio.app/Contents/jbr/Contents/Home")
    elif command -v javac >/dev/null 2>&1; then
        _jdks+=("$(dirname "$(dirname "$(readlink -f "$(command -v javac)")")")")
    fi
    for _jdk in ${_jdks[@]+"${_jdks[@]}"}; do
        if [ -n "$_jdk" ] && [ -x "$_jdk/bin/javac" ]; then
            export JAVA_HOME="$_jdk"
            break
        fi
    done
    unset _jdks _jdk
fi

if [ "${ANDROID_ENV_NO_NDK:-}" != 1 ]; then
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
fi
