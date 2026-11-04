// This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License.

package org.moqui.mobile

import android.content.Context
import android.util.Log

/**
 * Native push, when the build has it: Firebase Cloud Messaging is compiled in only when
 * `gen/android/app/google-services.json` exists (`src/fcm`, app/build.gradle.kts), so this asks for its class by name
 * and a build without it runs with push off. The token goes to Rust ([token]), which registers it with each account's
 * server (`/qapps/registerClientPush`); a push's data goes to [CallManager].
 */
object Push {
    private const val TAG = "moqui-push"

    /** Ask Firebase for the current token (it also comes by `onNewToken` when it changes). */
    fun start(context: Context) {
        if (!BuildConfig.FCM) {
            Log.i(TAG, "This build has no Firebase project (no google-services.json): push is off")
            return
        }
        try {
            Class.forName("org.moqui.mobile.FcmPush").getMethod("start", Context::class.java).invoke(null, context)
        } catch (err: Throwable) {
            Log.e(TAG, "Firebase did not start: push is off", err)
        }
    }

    /** A token, from Firebase (or the debug build's injection): kept and registered by Rust. */
    fun token(context: Context, token: String) {
        val app = context.applicationContext
        Native.work {
            if (Native.ensure(app)) Native.pushToken(Native.dataDir(app), token)
            else Log.w(TAG, "The app's library is not ready: the token is not registered")
        }
    }
}
