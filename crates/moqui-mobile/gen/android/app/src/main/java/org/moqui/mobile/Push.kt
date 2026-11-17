// This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License.

package org.moqui.mobile

import android.content.Context
import android.os.Handler
import android.os.Looper
import android.util.Log
import org.json.JSONObject

/**
 * Native push, when the build has it: Firebase Cloud Messaging is compiled in only when
 * `gen/android/app/google-services.json` exists (`src/fcm`, app/build.gradle.kts), so this asks for its class by name
 * and a build without it runs with push off. The token goes to Rust ([token]), which registers it, with the app's push
 * keys, with each account's server (`/qapps/registerClientPush`), tried again until each answers ([RegisterWorker]).
 * A push's data comes to [onMessage]: a call's ring and its end to [CallManager] (opened first when it is encrypted,
 * so its caller is never in clear text at Google), the rest to [ChatNotifications].
 */
object Push {
    private const val TAG = "moqui-push"
    private val main = Handler(Looper.getMainLooper())

    /** Ask Firebase for the current token (it also comes by `onNewToken` when it changes); register what waits. */
    fun start(context: Context) {
        ChatNotifications.ensureChannels(context)
        RegisterWorker.start(context)
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

    /** A token, from Firebase (or the debug build's injection): kept and registered by Rust, tried again until done. */
    fun token(context: Context, token: String) {
        val app = context.applicationContext
        Native.work {
            if (Native.ensure(app)) Native.pushToken(Native.dataDir(app), token)
            else Log.w(TAG, "The app's library is not ready: the token is not registered")
            RegisterWorker.start(app)
        }
    }

    /** A push's data (FCM's, or a test's): a ring or its end, else a chat push or a notification. */
    fun onMessage(context: Context, data: Map<String, String>) {
        val app = context.applicationContext
        val kind = data["kind"]
        val enc = data["enc"]
        if (kind != "ring" && kind != "ringEnded") {
            ChatNotifications.onPush(app, data)
            return
        }
        if (enc == null) {
            CallManager.onPush(app, data)
            return
        }
        // the ring's caller and call are sealed to the app: opened before it rings
        Native.work {
            val opened = if (Native.ensure(app)) Native.openPush(Native.dataDir(app), data["pushId"] ?: "", enc) else null
            if (opened == null) {
                Log.w(TAG, "A ring that does not open is not rung")
                return@work
            }
            val fields = HashMap<String, String>(data)
            fields.remove("enc")
            val json = JSONObject(opened)
            for (name in json.keys()) {
                if (name != "account") fields[name] = json.get(name).toString()
            }
            main.post { CallManager.onPush(app, fields) }
        }
    }
}
