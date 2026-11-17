// This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License.

package org.moqui.mobile

import android.content.Context
import android.os.Handler
import android.os.Looper
import android.util.Log
import org.json.JSONObject

/**
 * Native push. Firebase Cloud Messaging is compiled in only when `gen/android/app/google-services.json` exists
 * (`src/fcm`, app/build.gradle.kts), so this asks for its class by name and a build without it runs with FCM off;
 * UnifiedPush (a distributor such as ntfy, on a phone without Google Play) is always in ([UnifiedPushes]). The FCM token
 * goes to Rust ([token]), which registers it with each account's server (`/qapps/registerClientPush`) with that
 * account's own push keys, tried again until each answers ([RegisterWorker]). An FCM push's data comes to [onMessage]:
 * a call's ring and its end to [CallManager] (opened first when it is sealed, so its caller is never in clear text at
 * Google, and by the account whose keys open it), the rest to [ChatNotifications]. A UnifiedPush message comes opened
 * ([UnifiedPushService]) to [onOpened].
 */
object Push {
    private const val TAG = "moqui-push"
    private val main = Handler(Looper.getMainLooper())

    /**
     * Ask Firebase for the current token (it also comes by `onNewToken` when it changes). What waits is registered by
     * the app's start in Rust, which schedules [RegisterWorker] only when a registration fails (a worker scheduled at
     * every start would start the process again after it ended).
     */
    fun start(context: Context) {
        ChatNotifications.ensureChannels(context)
        UnifiedPushes.start(context)
        if (!BuildConfig.FCM) {
            Log.i(TAG, "This build has no Firebase project (no google-services.json): FCM is off")
            return
        }
        try {
            Class.forName("org.moqui.mobile.FcmPush").getMethod("start", Context::class.java).invoke(null, context)
        } catch (err: Throwable) {
            Log.e(TAG, "Firebase did not start: FCM is off", err)
        }
    }

    /** A token, from Firebase (or the debug build's injection): kept and registered by Rust, tried again until done. */
    fun token(context: Context, token: String) {
        val app = context.applicationContext
        Native.work {
            if (!Native.ensure(app)) {
                Log.w(TAG, "The app's library is not ready: the token is registered later")
                RegisterWorker.start(app)
                return@work
            }
            Native.pushToken(Native.dataDir(app), token)
            // an account its server did not answer yet: tried again until it does
            if (!Native.registerPending(Native.dataDir(app))) RegisterWorker.start(app)
        }
    }

    /** An FCM push's data (or a test's): a ring or its end, else a chat push or a notification. */
    fun onMessage(context: Context, data: Map<String, String>) {
        val app = context.applicationContext
        val received = System.currentTimeMillis()
        val kind = data["kind"] ?: ""
        val enc = data["enc"]
        if (kind != "ring" && kind != "ringEnded") {
            ChatNotifications.onPush(app, data)
            return
        }
        if (enc == null) {
            CallManager.onPush(app, data, null, received)
            return
        }
        // the ring's caller and call are sealed to the account's keys: opened (on the opener's thread, never behind a
        // registration's posts) before it rings
        Native.open {
            val opened = if (Native.ensure(app)) Native.openPush(Native.dataDir(app), data["pushId"] ?: "", kind, enc) else null
            val json = try {
                opened?.let { JSONObject(it) }
            } catch (err: Exception) {
                null
            }
            if (json == null) {
                Log.w(TAG, "A $kind that does not open: a missed call is shown in its place")
                if (kind == "ring") main.post { ChatNotifications.ringNotOpened(app) }
                return@open
            }
            main.post { onOpened(app, json, received) }
        }
    }

    /**
     * A push opened with an account's keys (its JSON, with that `account`): a ring or its end to [CallManager], with the
     * account the keys proved; anything else to [ChatNotifications].
     */
    fun onOpened(context: Context, opened: JSONObject, received: Long = System.currentTimeMillis()) {
        val app = context.applicationContext
        val kind = opened.optString("kind", "")
        if (kind != "ring" && kind != "ringEnded") {
            ChatNotifications.show(app, opened)
            return
        }
        val account = ChatRules.accountId(opened.optString("account", "")) ?: return
        val fields = HashMap<String, String>()
        for (name in opened.keys()) {
            if (name != "account") fields[name] = opened.get(name).toString()
        }
        CallManager.onPush(app, fields, account, received)
    }
}
