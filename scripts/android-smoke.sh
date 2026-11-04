#!/usr/bin/env bash
# A short smoke run of the mobile app on the emulator or a phone over adb (crates/moqui-mobile, plans/Mobile-Desktop.md
# MD6 MT1):
#   - install the debug APK, start the app fresh, wait for the shell's ready line in logcat ("moqui-mobile: the shell
#     loaded", logged once the shell's page loaded, with no server needed), and take a screenshot;
#   - the frame check (scripts/android-frames.py, over the WebView's DevTools socket of a debug build): the page has
#     the IPC, and a frame of another origin it adds has none (skipped without python3's websocket-client);
#   - back out of the app and start it again: the process ends with the activity, and the new one loads the shell.
# With a server on this machine's 8080, `adb reverse` lets the app reach it as http://localhost:8080 for a sign-in by
# hand. Not part of ./moq test-all; run it when asked:
#   ./moq android emulator && ./moq android build --target x86_64 --apk
#   scripts/android-smoke.sh [screenshot.png]
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

echo "== installing $apk on $serial" >&2
adb install -r "$apk" >/dev/null
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
pid=$(adb shell pidof "$package" | tr -d '\r')
if ! python3 -c 'import websocket' 2>/dev/null; then
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
if [ -n "$(adb shell pidof "$package" | tr -d '\r')" ]; then
    echo "FAILED: the process stayed after its activity ended" >&2
    failed=1
fi
# the first run's log is kept, and the relaunch's ready line is its own
log=$(adb logcat -d -s moqui-mobile)
adb logcat -c
adb shell am start -W -n "$package/.MainActivity" >/dev/null
wait_shell 1 || { echo "FAILED: the shell did not load again after a relaunch" >&2; failed=1; }
log=$(printf '%s\n%s\n' "$log" "$(adb logcat -d -s moqui-mobile)")

printf '%s\n' "$log" | grep -v 'moqui_client_tauri::scheme:' | sed 's/^/    /' >&2
if printf '%s\n' "$log" | grep -q ' E moqui-mobile'; then
    echo "FAILED: the app logged an error" >&2
    failed=1
fi
if [ -n "$failed" ]; then
    echo "FAILED (screenshot: $shot)" >&2
    exit 1
fi
echo "ok: the shell loaded, frames have no IPC, a relaunch after backing out loads it again (screenshot: $shot)" >&2
