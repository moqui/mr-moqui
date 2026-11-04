#!/usr/bin/env bash
# A short smoke run of the mobile app on the emulator or a phone over adb (crates/moqui-mobile, plans/Mobile-Desktop.md
# MD6 MT1): install the debug APK, start the app fresh, wait for the shell's ready line in logcat ("moqui-mobile: the
# shell loaded", which the app logs once the shell's page loaded, with no server needed), and take a screenshot. With
# a server on this machine's 8080, `adb reverse` lets the app reach it as http://localhost:8080 for a sign-in by hand.
# Not part of ./moq test-all; run it when asked:
#   ./moq android emulator && ./moq android build --target x86_64 --apk
#   scripts/android-smoke.sh [screenshot.png]
# ANDROID_SERIAL names the device (default: the first adb lists); MOQUI_MOBILE_APK the APK (default: the universal
# debug APK of ./moq android build --apk).
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
echo "== installing $apk on $serial" >&2
adb install -r "$apk" >/dev/null
adb shell am force-stop "$package"
adb logcat -c
# the server on this machine, if one runs, as the app's http://localhost:8080
adb reverse tcp:8080 tcp:8080 >/dev/null || true
echo "== starting $package" >&2
adb shell am start -W -n "$package/.MainActivity" >/dev/null
ready=
for _ in $(seq 1 30); do
    if adb logcat -d -s moqui-mobile | grep -q 'moqui-mobile: the shell loaded'; then
        ready=1
        break
    fi
    sleep 1
done
mkdir -p "$(dirname "$shot")"
adb exec-out screencap -p >"$shot"
adb logcat -d -s moqui-mobile | sed 's/^/    /' >&2
if [ -z "$ready" ]; then
    echo "FAILED: the shell did not load in 30 seconds (screenshot: $shot)" >&2
    exit 1
fi
if adb logcat -d -s moqui-mobile | grep -q ' E moqui-mobile'; then
    echo "FAILED: the app logged an error (screenshot: $shot)" >&2
    exit 1
fi
echo "ok: the shell loaded (screenshot: $shot)" >&2
