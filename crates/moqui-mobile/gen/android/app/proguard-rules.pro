# Add project specific ProGuard rules here.
# You can control the set of applied configuration files using the
# proguardFiles setting in build.gradle.
#
# For more details, see
#   http://developer.android.com/guide/developing/tools/proguard.html

# If your project uses WebView with JS, uncomment the following
# and specify the fully qualified class name to the JavaScript interface
# class:
#-keepclassmembers class fqcn.of.javascript.interface.for.webview {
#   public *;
#}

# Uncomment this to preserve the line number information for
# debugging stack traces.
#-keepattributes SourceFile,LineNumberTable

# If you keep the line number information, uncomment this to
# hide the original source file name.
#-renamesourcefileattribute SourceFile

# rustls-platform-verifier's Kotlin component, called only through JNI.
-keep, includedescriptorclasses class org.rustls.platformverifier.** { *; }

# The device keys' Keystore vault (src/main/java/org/moqui/mobile/KeyVault.kt), called only through JNI.
-keep class org.moqui.mobile.KeyVault { *; }

# The native code Rust calls (the page's call state) and the library's entry points (src/native.rs).
-keep class org.moqui.mobile.CallManager { *; }
-keep class org.moqui.mobile.Native { *; }
-keep class org.moqui.mobile.ActivityEnd { *; }
# Asked for by name (Push.kt) in a build with Firebase.
-keep class org.moqui.mobile.FcmPush { *; }

# WorkManager (FlushWorker.kt): Room makes WorkManager's database by reflection on its no-argument constructor, which
# room-runtime 2.6's own rule (-keep class * extends RoomDatabase) does not keep under R8's full mode: without this, a
# release build ends at its start (NoSuchMethodException: WorkDatabase_Impl.<init>). WorkManager makes the worker by
# its (Context, WorkerParameters) constructor.
-keep class * extends androidx.room.RoomDatabase { <init>(); }
-keep class org.moqui.mobile.FlushWorker { public <init>(android.content.Context, androidx.work.WorkerParameters); }
