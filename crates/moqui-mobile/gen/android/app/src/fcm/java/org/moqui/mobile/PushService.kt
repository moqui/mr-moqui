// This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License.

package org.moqui.mobile

import android.content.Context
import android.util.Log
import com.google.firebase.installations.FirebaseInstallations
import com.google.firebase.messaging.FirebaseMessaging
import com.google.firebase.messaging.FirebaseMessagingService
import com.google.firebase.messaging.RemoteMessage

/**
 * Firebase Cloud Messaging, compiled in only when the build has `google-services.json` (app/build.gradle.kts): the
 * server's data messages (a ring, its end, a chat push) reach [Push.onMessage] with no WebView, and the token goes to
 * Rust ([Push]).
 */
class PushService : FirebaseMessagingService() {
    override fun onNewToken(token: String) {
        Push.token(this, token)
    }

    override fun onMessageReceived(message: RemoteMessage) {
        // a high-priority message lets the app start its call service from the background (an FCM exemption)
        Log.i("moqui-push", "A push of kind ${message.data["kind"]} (priority ${message.priority})")
        Push.onMessage(this, message.data)
    }
}

/** Called by name from [Push.startFcm] and [Push.stopFcm]. */
object FcmPush {
    /** The current token, which Rust registers if it is new. */
    @JvmStatic
    fun start(context: Context) {
        FirebaseMessaging.getInstance().token
            .addOnSuccessListener { token -> Push.token(context, token) }
            .addOnFailureListener { err -> Log.w("moqui-push", "No FCM token: ${err.message}") }
    }

    /**
     * No account is signed in here any more (SEC-CLI-121): the token is deleted (FCM takes no push for it), then the
     * Firebase installation, which the token's deletion still needs; with auto-init off neither is made again until a
     * sign-in asks ([start]).
     */
    @JvmStatic
    @Suppress("UNUSED_PARAMETER")
    fun stop(context: Context) {
        FirebaseMessaging.getInstance().deleteToken().addOnCompleteListener { deleted ->
            if (!deleted.isSuccessful) Log.w("moqui-push", "The FCM token was not deleted: ${deleted.exception?.message}")
            FirebaseInstallations.getInstance().delete().addOnCompleteListener { gone ->
                if (gone.isSuccessful) Log.i("moqui-push", "No account is signed in: Firebase's token and installation are deleted")
                else Log.w("moqui-push", "The Firebase installation was not deleted: ${gone.exception?.message}")
            }
        }
    }
}
