// This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License.

package org.moqui.mobile

import android.content.Context
import android.os.Handler
import android.os.Looper
import android.util.Log
import org.json.JSONObject
import org.unifiedpush.android.connector.FailedReason
import org.unifiedpush.android.connector.PushService
import org.unifiedpush.android.connector.data.PushEndpoint
import org.unifiedpush.android.connector.data.PushMessage

/**
 * The UnifiedPush connector's events ([UnifiedPushes]): an account's endpoint goes to Rust, which registers it with the
 * account's server with the account's own push keys; a message is opened here with those keys (`Native.openFor`) and is
 * that account's (the instance), shown as an FCM push is once opened ([Push.onOpened]).
 *
 * The connector's own receiver (`MessagingReceiverImpl`) tries its default key manager, which has no keys of the
 * instance, so the message comes still sealed (`decrypted` false), and this opens it. One the connector says it opened
 * was sealed to the connector's own keys, which no server was given: it is dropped (logged), as is one that does not
 * open with the account's keys.
 */
class UnifiedPushService : PushService() {
    override fun onNewEndpoint(endpoint: PushEndpoint, instance: String) {
        val app = applicationContext
        val account = ChatRules.accountId(instance) ?: return
        Native.work {
            if (Native.ensure(app)) Native.unifiedEndpoint(Native.dataDir(app), account, endpoint.url)
        }
    }

    override fun onMessage(message: PushMessage, instance: String) {
        val app = applicationContext
        handle(app, message, instance) { sealed ->
            if (Native.ensure(app)) Native.openFor(Native.dataDir(app), instance, sealed) else null
        }
    }

    override fun onRegistrationFailed(reason: FailedReason, instance: String) {
        Log.w(TAG, "UnifiedPush did not register $instance: $reason")
    }

    override fun onUnregistered(instance: String) {
        val app = applicationContext
        val account = ChatRules.accountId(instance) ?: return
        Native.work {
            if (Native.ensure(app)) Native.unifiedEndpoint(Native.dataDir(app), account, "")
        }
    }

    companion object {
        private const val TAG = "moqui-push"
        private val main = Handler(Looper.getMainLooper())

        /**
         * A message for the account `instance`: opened with `open` (the account's keys, on the opener's thread), then
         * shown; one that does not open, or that the connector opened with its own keys, is dropped.
         */
        fun handle(context: Context, message: PushMessage, instance: String, open: (ByteArray) -> ByteArray?) {
            val app = context.applicationContext
            val received = System.currentTimeMillis()
            PushCheck.arrived(app, "unifiedpush")
            val account = ChatRules.accountId(instance) ?: return
            if (message.decrypted) {
                Log.w(TAG, "A UnifiedPush message for $account opened with the connector's own keys: dropped")
                return
            }
            val sealed = message.content
            Native.open {
                val plain = open(sealed)
                val opened = try {
                    plain?.let { JSONObject(String(it, Charsets.UTF_8)) }
                } catch (err: Exception) {
                    null
                }
                if (opened == null) {
                    Log.w(TAG, "A UnifiedPush message for $account that does not open with its keys: dropped")
                    return@open
                }
                opened.put("account", account)
                main.post { Push.onOpened(app, opened, received) }
            }
        }
    }
}
