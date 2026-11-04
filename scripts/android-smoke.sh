#!/usr/bin/env bash
# A short smoke run of the mobile app on the emulator or a phone over adb (crates/moqui-mobile, plans/Mobile-Desktop.md
# MD6 MT1):
#   - install the debug APK, start the app fresh, wait for the shell's ready line in logcat ("moqui-mobile: the shell
#     loaded", logged once the shell's page loaded, with no server needed), and take a screenshot;
#   - the frame check (scripts/android-frames.py, over the WebView's DevTools socket of a debug build): the page has
#     the IPC, and a frame of another origin it adds has none (skipped without python3's websocket-client);
#   - back out of the app and start it again: the process ends with the activity, and the new one loads the shell.
# A release APK (not debuggable, so no DevTools socket) skips the frame check; instead the classes and methods Rust
# calls by name through JNI are looked for in its dex files (R8 must keep them: proguard-rules.pro, wry's
# proguard-wry.pro), and the log must have no JNI lookup failure (UnsatisfiedLinkError, NoSuchMethodError, ...).
# With a server on this machine's 8080, `adb reverse` lets the app reach it as http://localhost:8080 for a sign-in by
# hand. Not part of ./moq test-all; run it when asked:
#   ./moq android emulator && ./moq android build --target x86_64 --apk
#   scripts/android-smoke.sh [screenshot.png]
#   MOQUI_MOBILE_APK=crates/moqui-mobile/gen/android/app/build/outputs/apk/universal/release/app-universal-release.apk \
#       scripts/android-smoke.sh      # a signed release build (./moq android build --release --target x86_64 --apk)
# ANDROID_SERIAL names the device (default: the first adb lists); MOQUI_MOBILE_APK the APK (default: the universal
# debug APK of ./moq android build --apk); MOQUI_SMOKE_APP a server path (/qapps/...) the frame check opens first, with
# an account signed in, so the frames go into a server page.
set -euo pipefail
cd "$(dirname "$0")/.."
shot=${1:-target/android-smoke.png}
apk=${MOQUI_MOBILE_APK:-crates/moqui-mobile/gen/android/app/build/outputs/apk/universal/debug/app-universal-debug.apk}
sdk=${ANDROID_HOME:-$HOME/Android/Sdk}
adb_bin=$sdk/platform-tools/adb
[ -x "$adb_bin" ] || { echo "no adb in $sdk (ANDROID_HOME)" >&2; exit 1; }
serial=${ANDROID_SERIAL:-$("$adb_bin" devices | sed -n 's/^\([^[:space:]]*\)[[:space:]]*device$/\1/p' | head -1)}
[ -n "$serial" ] || { echo "no device: ./moq android emulator, or a phone over adb" >&2; exit 1; }
adb() { "$adb_bin" -s "$serial" "$@"; }
[ -f "$apk" ] || { echo "no APK at $apk: ./moq android build --target x86_64 --apk" >&2; exit 1; }
package=org.moqui.mobile
failed=

# Wait for the shell's ready line; $1 is how many such lines the log must have.
wait_shell() {
    local _
    for _ in $(seq 1 30); do
        [ "$(adb logcat -d -s moqui-mobile | grep -c 'moqui-mobile: the shell loaded')" -ge "$1" ] && return 0
        sleep 1
    done
    return 1
}

# The classes and methods Rust calls by name (src/android.rs, src/native.rs, wry's main_pipe.rs, tao's ndk_glue.rs).
JNI_NAMES=(
    org/moqui/mobile/KeyVault#ready org/moqui/mobile/KeyVault#seal org/moqui/mobile/KeyVault#open
    org/moqui/mobile/CallManager#pageCallState org/moqui/mobile/ActivityEnd#endedForGood
    org/moqui/mobile/Native#init org/moqui/mobile/Native#pushToken org/moqui/mobile/Native#decline
    org/moqui/mobile/Native#hasAccount org/moqui/mobile/Native#flush
    org/moqui/mobile/Ipc#attach org/moqui/mobile/RustWebView#loadUrlMainThread org/moqui/mobile/RustWebView#evalScript
    org/moqui/mobile/WryActivity#getAppClass org/moqui/mobile/WryActivity#getVersion
    org/moqui/mobile/WryActivity#setWebView org/rustls/platformverifier/CertificateVerifier#verifyCertificateChain
)

# Each of JNI_NAMES in the APK's dex files, by its own name (not renamed or removed by R8).
check_jni_names() {
    local dexdump tmp have name missing=
    dexdump=$(find "$sdk/build-tools" -name dexdump -type f 2>/dev/null | sort -V | tail -1)
    if [ -z "$dexdump" ] || ! command -v unzip >/dev/null; then
        echo "   skipped: no dexdump in $sdk/build-tools, or no unzip" >&2
        return 0
    fi
    tmp=$(mktemp -d)
    unzip -q -o "$apk" 'classes*.dex' -d "$tmp"
    have=$("$dexdump" "$tmp"/classes*.dex 2>/dev/null | awk '
        /Class descriptor/ { cls = $4; gsub(/^.L|;.$/, "", cls) }
        /^      name  / { n = $3; gsub(/'"'"'/, "", n); print cls "#" n }' | sort -u)
    rm -rf "$tmp"
    for name in "${JNI_NAMES[@]}"; do
        grep -qxF "$name" <<<"$have" || missing="$missing $name"
    done
    if [ -n "$missing" ]; then
        echo "FAILED: R8 removed or renamed what Rust calls by name:$missing" >&2
        return 1
    fi
    echo "   the ${#JNI_NAMES[@]} classes and methods Rust calls by name are in the dex files" >&2
}

echo "== installing $apk on $serial" >&2
adb install -r "$apk" >/dev/null
debuggable=
flags=$(adb shell dumpsys package "$package" | grep 'flags=\[' || true)
case $flags in *DEBUGGABLE*) debuggable=1 ;; esac
if [ -z "$debuggable" ]; then
    echo "== a release build: what R8 kept for JNI" >&2
    check_jni_names || failed=1
fi
adb shell am force-stop "$package"
adb logcat -c
# the server on this machine, if one runs, as the app's http://localhost:8080
adb reverse tcp:8080 tcp:8080 >/dev/null || true
echo "== starting $package" >&2
adb shell am start -W -n "$package/.MainActivity" >/dev/null
wait_shell 1 || { echo "FAILED: the shell did not load in 30 seconds" >&2; failed=1; }
mkdir -p "$(dirname "$shot")"
adb exec-out screencap -p >"$shot"

echo "== frames: only the main frame has the IPC" >&2
pid=$(adb shell pidof "$package" | tr -d '\r' || true)
if [ -z "$debuggable" ]; then
    echo "   skipped: a release build has no DevTools socket" >&2
elif ! python3 -c 'import websocket' 2>/dev/null; then
    echo "   skipped: python3 has no websocket-client" >&2
elif [ -z "$pid" ]; then
    echo "FAILED: the app is not running" >&2
    failed=1
else
    port=9229
    adb forward "tcp:$port" "localabstract:webview_devtools_remote_$pid" >/dev/null
    python3 scripts/android-frames.py "$port" ${MOQUI_SMOKE_APP:+"$MOQUI_SMOKE_APP"} >&2 || failed=1
    adb forward --remove "tcp:$port" >/dev/null || true
fi

echo "== backing out of the app, and starting it again" >&2
for _ in 1 2 3 4; do
    adb shell input keyevent 4
    sleep 1
done
sleep 2
if [ -n "$(adb shell pidof "$package" | tr -d '\r' || true)" ]; then
    echo "FAILED: the process stayed after its activity ended" >&2
    failed=1
fi
# the first run's log is kept, and the relaunch's ready line is its own
log=$(adb logcat -d -s moqui-mobile)
jni_failures=$(adb logcat -d | grep -E 'UnsatisfiedLinkError|NoSuchMethod(Error|Exception)|NoClassDefFoundError|ClassNotFoundException: org\.(moqui|rustls)|Process: org\.moqui\.mobile,' || true)
adb logcat -c
adb shell am start -W -n "$package/.MainActivity" >/dev/null
wait_shell 1 || { echo "FAILED: the shell did not load again after a relaunch" >&2; failed=1; }
log=$(printf '%s\n%s\n' "$log" "$(adb logcat -d -s moqui-mobile)")
jni_failures=$(printf '%s\n%s' "$jni_failures" "$(adb logcat -d | grep -E 'UnsatisfiedLinkError|NoSuchMethod(Error|Exception)|NoClassDefFoundError|ClassNotFoundException: org\.(moqui|rustls)|Process: org\.moqui\.mobile,' || true)" | sed '/^$/d')

printf '%s\n' "$log" | grep -v 'moqui_client_tauri::scheme:' | sed 's/^/    /' >&2
if printf '%s\n' "$log" | grep -q ' E moqui-mobile'; then
    echo "FAILED: the app logged an error" >&2
    failed=1
fi
if [ -n "$jni_failures" ]; then
    printf '%s\n' "$jni_failures" | sed 's/^/    /' >&2
    echo "FAILED: the app crashed, or a class or method was not found" >&2
    failed=1
fi
if [ -n "$failed" ]; then
    echo "FAILED (screenshot: $shot)" >&2
    exit 1
fi
echo "ok: the shell loaded, frames have no IPC, a relaunch after backing out loads it again (screenshot: $shot)" >&2
