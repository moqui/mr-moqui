#!/usr/bin/env bash
# LocationShareWalk's phone half, the acceptance walk of live location on the emulator or a phone over adb
# (plans/archive/Location-Sharing.md LL7; docs/tools/Maps.md, Live location; docs/ui/Mobile-Client.md, Live location):
#   - the server half: the tools acceptance spec LocationShareWalkFixtures (loc.walk.ann, loc.walk.ben and loc.walk.cy in
#     Walk family) run by `moqui test --serve` on a scratch database, served on 127.0.0.1:$port, which the app reaches
#     through `adb reverse` as http://localhost:$port;
#   - the device half: the debug APK installed with its runtime permissions granted and its data cleared, started, and
#     scripts/android-location-share.py over the WebView's DevTools socket and Android's own UI (uiautomator): sign in
#     as Ann, Maps' Share my location with Walk family, the app's confirmation dialog answered Send, its foreground
#     notification, the position Ben is given (also after the app went to the background), and the notification's
#     Stop ending the share.
# The browser half (three browsers, the push, the extension, a member taken out) is the client acceptance spec
# LocationShareWalk (./moq accept tools --spec LocationShareWalk). Not part of ./moq test-all or accept-all; run it when
# asked or when the walk changes:
#   ./moq android emulator && ./moq android build --target x86_64 --apk
#   scripts/android-location-share.sh
# On a real phone it wipes the app's data (pm clear) first: its accounts and everything kept for them.
# ANDROID_SERIAL names the device (default: the first adb lists); MOQUI_MOBILE_APK the APK (default: the universal debug
# APK of ./moq android build --apk); MOQUI_WALK_PORT the server's port (18087); MOQUI_WALK_PACKAGE the moq package of
# the server binary (release, target/release/moqui, by default; debug for ./moq build --debug). The server's log is
# target/location-walk/server.log, the last screen target/location-walk/last.png.
set -euo pipefail
cd "$(dirname "$0")/.."
started=$(date +%s)
port=${MOQUI_WALK_PORT:-18087}
devtools=9232
package_opt=--${MOQUI_WALK_PACKAGE:-release}
apk=${MOQUI_MOBILE_APK:-crates/moqui-mobile/gen/android/app/build/outputs/apk/universal/debug/app-universal-debug.apk}
sdk=${ANDROID_HOME:-$HOME/Android/Sdk}
adb_bin=$sdk/platform-tools/adb
out=target/location-walk
app=org.moqui.mobile
[ -x "$adb_bin" ] || { echo "no adb in $sdk (ANDROID_HOME)" >&2; exit 1; }
serial=${ANDROID_SERIAL:-$("$adb_bin" devices | sed -n 's/^\([^[:space:]]*\)[[:space:]]*device$/\1/p' | head -1)}
[ -n "$serial" ] || { echo "no device: ./moq android emulator, or a phone over adb" >&2; exit 1; }
[ -f "$apk" ] || { echo "no APK at $apk: ./moq android build --target x86_64 --apk" >&2; exit 1; }
# -P (Python 3.11 and later) leaves the script's directory off sys.path and keeps the user's site-packages
python=(python3 -P)
"${python[@]}" -c '' 2>/dev/null || python=(python3)
"${python[@]}" -c 'import websocket' 2>/dev/null || { echo "python3 needs websocket-client (pip install websocket-client)" >&2; exit 1; }
# fd 3 (the server's standard input, below) closed for each child, so none holds it open past this script
adb() { "$adb_bin" -s "$serial" "$@" 3>&-; }
elapsed() { echo "$(($(date +%s) - started))"; }

rm -rf "$out"
mkdir -p "$out"
# The server: its standard input a pipe this script holds open, so it stops when the script ends however it ends
mkfifo "$out/stdin"
echo "== the server half: moqui test framework tools --acceptance --spec LocationShareWalkFixtures --serve 127.0.0.1:$port" >&2
./moq test "$package_opt" --nobuild framework tools --acceptance --spec LocationShareWalkFixtures --db snapshot \
    --serve "127.0.0.1:$port" --ready-file "$out/ready" <"$out/stdin" >"$out/server.log" 2>&1 &
server=$!
exec 3>"$out/stdin"
# however the script ends (a signal to the walk too): the notification shade closed, the app force-stopped unless the
# walk passed (so a share it left on is not sent from the device any more), the walk's UI dump removed, the forwards
# gone, and the server stopped by the end of its standard input, else by SIGTERM after 30 seconds and SIGKILL 10
# seconds later; a second signal does not cut the cleanup short
passed=0
cleanup() {
    trap '' INT TERM HUP
    exec 3>&-
    adb shell cmd statusbar collapse >/dev/null 2>&1 || true
    [ "$passed" = 1 ] || adb shell am force-stop "$app" >/dev/null 2>&1 || true
    adb shell rm -f /sdcard/location-walk-ui.xml >/dev/null 2>&1 || true
    adb reverse --remove "tcp:$port" >/dev/null 2>&1 || true
    adb forward --remove "tcp:$devtools" >/dev/null 2>&1 || true
    for _ in $(seq 1 30); do
        kill -0 "$server" 2>/dev/null || break
        sleep 1
    done
    kill "$server" 2>/dev/null || true
    for _ in $(seq 1 10); do
        kill -0 "$server" 2>/dev/null || break
        sleep 1
    done
    kill -KILL "$server" 2>/dev/null || true
    wait "$server" 2>/dev/null || true
}
trap cleanup EXIT
trap 'exit 1' INT TERM HUP
# a first run loads the data into a snapshot (minutes); later ones restore it (seconds)
for _ in $(seq 1 1800); do
    [ -s "$out/ready" ] && break
    kill -0 "$server" 2>/dev/null || { tail -30 "$out/server.log" >&2; echo "FAILED: the server half ended (its log above)" >&2; exit 1; }
    sleep 1
done
[ -s "$out/ready" ] || { echo "FAILED: the server did not start in 30 minutes" >&2; exit 1; }
grep -E 'LocationShareWalkFixtures|PASSED|FAILED' "$out/server.log" | grep -v '^\s*$' | tail -5 | sed 's/^/   /' >&2 || true
echo "   serving $(head -1 "$out/ready") after $(elapsed) s" >&2

echo "== installing $apk on $serial, its data cleared" >&2
case "$serial" in
emulator-*) ;;
*) echo "warning: $serial is not an emulator: pm clear wipes all of this app's data on it" >&2 ;;
esac
adb shell pm clear "$app" >/dev/null 2>&1 || true
adb install -r -g "$apk" >/dev/null
adb reverse "tcp:$port" "tcp:$port" >/dev/null
adb logcat -c
adb shell am start -W -n "$app/.MainActivity" >/dev/null
for _ in $(seq 1 30); do
    adb logcat -d -s moqui-mobile | grep -q 'moqui-mobile: the shell loaded' && break
    sleep 1
done
adb shell pidof "$app" >/dev/null || { echo "FAILED: the app is not running" >&2; exit 1; }

echo "== the device half (after $(elapsed) s)" >&2
status=0
"${python[@]}" scripts/android-location-share.py --devtools "$devtools" --serial "$serial" --adb "$adb_bin" --port "$port" \
    --password 'LocationWalk1!!' 3>&- || status=$?
adb exec-out screencap -p >"$out/last.png" || true
if [ "$status" -ne 0 ]; then
    adb logcat -d -s moqui-mobile | grep -E ' [EW] ' | tail -20 | sed 's/^/    /' >&2 || true
    echo "FAILED: LocationShareWalk (the phone) after $(elapsed) s (the server's log: $out/server.log, the screen: $out/last.png)" >&2
    exit 1
fi
passed=1
echo "LocationShareWalk (the phone) passed in $(elapsed) s" >&2
