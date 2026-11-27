#!/bin/sh
# Usage: [SYNC=true] measure.sh MODE [N]  MODE: native, web or s7. SYNC=true makes the WebView before the first frame. Needs the APK installed, the prototype server on the host
# port 18777 and `adb reverse tcp:18777 tcp:18777`. Prints per cold start: TotalTime (first frame) and Fully drawn
# (WebView content), in ms. Then warm starts (HOME, then start again): a hot start has no TotalTime, so it prints
# wNN, the WaitTime.
mode=$1; n=${2:-10}; adb=${ADB:-adb}; c=org.moqui.startproto/.Main
url=${URL:-http://localhost:18777/b/}
start() { $adb shell am start -W $1 -n $c --es mode "$mode" --es url "$url" --ez sync ${SYNC:-false} | tr -d '\r' | awk '/^TotalTime:/{t=$2} /^WaitTime:/{w=$2} END{print (t?t:"w"w)}'; }
drawn() { $adb logcat -m 1 -e "Fully drawn $c" | sed -n 's/.*: +\([0-9s]*\)ms.*/\1/p' | tr -d '\r'; }
fresh() { $adb logcat -c; }
# Prime: two starts, so the page, its libraries and the service worker are cached and a snapshot is saved.
for i in 1 2; do fresh; start -S >/dev/null; [ "$mode" = native ] || drawn >/dev/null; done
echo "cold $mode"
for i in $(seq "$n"); do
  $adb shell am force-stop org.moqui.startproto; fresh
  t=$(start -S); d=-; [ "$mode" = native ] || d=$(drawn); echo "$t $d"
done
echo "warm $mode"
for i in $(seq "$n"); do
  $adb shell input keyevent KEYCODE_HOME; fresh
  t=$(start ""); echo "$t -"
done
