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
-keep class org.moqui.mobile.CallAudio { *; }
-keep class org.moqui.mobile.ScreenShare { *; }
-keep class org.moqui.mobile.Native { *; }
-keep class org.moqui.mobile.ActivityEnd { *; }
# Rust calls the notification check by name (src/android.rs push_check_state, push_check_fix)
-keep class org.moqui.mobile.PushCheck { *; }
# ... and the chat notifications' clear and room shown, and UnifiedPush's registration (src/android.rs)
-keep class org.moqui.mobile.ChatNotifications { *; }
-keep class org.moqui.mobile.UnifiedPushes { *; }
# ... and the phone's contacts and the calling account (src/android.rs contacts_search, contact_names, dial_account)
-keep class org.moqui.mobile.PhoneBook { *; }
-keep class org.moqui.mobile.DialAccount { *; }
# ... and screen security and the calendar's new event (src/android.rs secure_screen, calendar_insert; SEC-CLI-127:
# without these R8 may remove or rename ScreenSecurity.set and CalendarIntent.insert in a release build)
-keep class org.moqui.mobile.ScreenSecurity { *; }
-keep class org.moqui.mobile.CalendarIntent { *; }
# ... and Firebase's start once an account signs in, and its token's deletion once none is (src/android.rs fcm_start,
# fcm_stop; SEC-CLI-121)
-keep class org.moqui.mobile.Push {
    public static void startFcm(android.content.Context);
    public static void stopFcm(android.content.Context);
}
# ... and the offline map downloads' worker (src/android.rs package_worker_start, package_worker_cancel; plans/Maps-Offline.md
# §2.4)
-keep class org.moqui.mobile.PackageDownloadWorker {
    public <init>(android.content.Context, androidx.work.WorkerParameters);
    public static java.lang.String start(android.content.Context, java.lang.String, java.lang.String, long, boolean);
    public static void cancel(android.content.Context, java.lang.String, java.lang.String);
    public static void cancelAccount(android.content.Context, java.lang.String);
}
# ... and whether the phone has a secure lock screen (src/android.rs device_secure; SEC-CLI-113)
-keep class org.moqui.mobile.DeviceLock { *; }
# The server pages' storage, cleared at a switch of account (PageStorage.kt, SEC-CLI-81), called only through JNI.
-keep class org.moqui.mobile.PageStorage { *; }
# ... and the server whose pages the app origin shows, for their dialogs' title (src/android.rs page_server; wry's
# RustWebChromeClient, crates/moqui-mobile/patches; SEC-CLI-119)
-keep class org.moqui.mobile.RustWebChromeClient {
    public static void pageServer(java.lang.String);
}
# Asked for by name (Push.kt) in a build with Firebase.
-keep class org.moqui.mobile.FcmPush { *; }

# WorkManager (FlushWorker.kt): Room makes WorkManager's database by reflection on its no-argument constructor, which
# room-runtime 2.6's own rule (-keep class * extends RoomDatabase) does not keep under R8's full mode: without this, a
# release build ends at its start (NoSuchMethodException: WorkDatabase_Impl.<init>). WorkManager makes the worker by
# its (Context, WorkerParameters) constructor.
-keep class * extends androidx.room.RoomDatabase { <init>(); }
-keep class org.moqui.mobile.FlushWorker { public <init>(android.content.Context, androidx.work.WorkerParameters); }
-keep class org.moqui.mobile.RegisterWorker {
    public <init>(android.content.Context, androidx.work.WorkerParameters);
    public static void start(android.content.Context);
}
