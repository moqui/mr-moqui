#!/bin/sh
# Build the start-time probe APK with no Gradle. Needs JAVA_HOME (a JDK) and the SDK's build-tools 35 and android-35.
set -e
sdk=${ANDROID_HOME:-$HOME/Android/Sdk}; bt=$sdk/build-tools/35.0.0; jar=$sdk/platforms/android-35/android.jar
here=$(cd "$(dirname "$0")" && pwd); out=${1:-$HOME/.cache/app-startup-apk}
rm -rf "$out" && mkdir -p "$out/classes"
"$bt/aapt2" link -o "$out/base.apk" -I "$jar" --manifest "$here/AndroidManifest.xml"
"$JAVA_HOME/bin/javac" -source 11 -target 11 -classpath "$jar" -d "$out/classes" "$here"/src/org/moqui/startproto/*.java 2>&1 | grep -v warning || true
"$bt/d8" --min-api 26 --lib "$jar" --output "$out" $(find "$out/classes" -name '*.class')
(cd "$out" && cp base.apk u.apk && zip -q u.apk classes.dex)
"$bt/zipalign" -f 4 "$out/u.apk" "$out/a.apk"
[ -f "$out/../app-startup.keystore" ] || "$JAVA_HOME/bin/keytool" -genkeypair -keystore "$out/../app-startup.keystore" -storepass android -keypass android -alias k -keyalg RSA -validity 3650 -dname CN=probe >/dev/null 2>&1
"$bt/apksigner" sign --ks "$out/../app-startup.keystore" --ks-pass pass:android --out "$out/startproto.apk" "$out/a.apk"
echo "$out/startproto.apk"
