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
     * At the app's start: the channels, UnifiedPush, and Firebase only when an account is signed in here ([startFcm]),
     * so a Firebase build reaches Google only once one is (SEC-CLI-121; `src/fcm`'s manifest turns FCM's own auto-init
     * off). What waits is registered by the app's start in Rust, which schedules [RegisterWorker] only when a
     * registration fails (a worker scheduled at every start would start the process again after it ended).
     */
    fun start(context: Context) {
        ChatNotifications.ensureChannels(context)
        UnifiedPushes.start(context)
        if (!BuildConfig.FCM) {
            Log.i(TAG, "This build has no Firebase project (no google-services.json): FCM is off")
            return
        }
        if (Native.accounts(context).isEmpty()) {
            Log.i(TAG, "No account is signed in here: Firebase starts at the first sign-in")
            return
        }
        startFcm(context)
    }

    /**
     * Ask Firebase for the current token (it also comes by `onNewToken` when it changes; an explicit ask works with
     * auto-init off): at the start with an account signed in ([start]), and from Rust when one signs in
     * (src/android.rs fcm_start). Nothing in a build without Firebase.
     */
    @JvmStatic
    fun startFcm(context: Context) {
        if (!BuildConfig.FCM) return
        try {
            Class.forName("org.moqui.mobile.FcmPush").getMethod("start", Context::class.java).invoke(null, context)
        } catch (err: Throwable) {
            Log.e(TAG, "Firebase did not start: FCM is off", err)
        }
    }

    /**
     * The last account here signed out (src/android.rs fcm_stop, SEC-CLI-121): Firebase's token and installation are
     * deleted ([FcmPush.stop]), so the phone stays registered with Google for no account, and Rust has forgotten the
     * token; the next sign-in asks for a new one ([startFcm]). Nothing in a build without Firebase.
     */
    @JvmStatic
    fun stopFcm(context: Context) {
        if (!BuildConfig.FCM) return
        try {
            Class.forName("org.moqui.mobile.FcmPush").getMethod("stop", Context::class.java).invoke(null, context)
        } catch (err: Throwable) {
            Log.e(TAG, "Firebase's token was not deleted", err)
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
                // a missed call in its place, only when the push names an account here (not another app's or a
                // signed-out account's)
                val known = kind == "ring" && Native.ensure(app) && Native.pushKnown(Native.dataDir(app), data["pushId"] ?: "")
                Log.w(TAG, "A $kind that does not open" + if (known) ": a missed call is shown in its place" else ": dropped")
                if (known) main.post { ChatNotifications.ringNotOpened(app) }
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
