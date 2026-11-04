// This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License.

package org.moqui.mobile

import android.content.Context
import android.util.Log
import com.google.firebase.messaging.FirebaseMessaging
import com.google.firebase.messaging.FirebaseMessagingService
import com.google.firebase.messaging.RemoteMessage

/**
 * Firebase Cloud Messaging, compiled in only when the build has `google-services.json` (app/build.gradle.kts): the
 * server's data messages (a ring, its end) reach [CallManager] with no WebView, and the token goes to Rust ([Push]).
 */
class PushService : FirebaseMessagingService() {
    override fun onNewToken(token: String) {
        Push.token(this, token)
    }

    override fun onMessageReceived(message: RemoteMessage) {
        // a high-priority message lets the app start its call service from the background (an FCM exemption)
        Log.i("moqui-push", "A push of kind ${message.data["kind"]} (priority ${message.priority})")
        CallManager.onPush(this, message.data)
    }
}

/** Called by name from [Push.start]: the current token, which Rust registers if it is new. */
object FcmPush {
    @JvmStatic
    fun start(context: Context) {
        FirebaseMessaging.getInstance().token
            .addOnSuccessListener { token -> Push.token(context, token) }
            .addOnFailureListener { err -> Log.w("moqui-push", "No FCM token: ${err.message}") }
    }
}
