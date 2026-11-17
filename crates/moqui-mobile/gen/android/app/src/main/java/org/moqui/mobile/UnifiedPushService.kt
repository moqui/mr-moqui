// This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License.

package org.moqui.mobile

import android.util.Log
import org.json.JSONObject
import org.unifiedpush.android.connector.FailedReason
import org.unifiedpush.android.connector.PushService
import org.unifiedpush.android.connector.data.PushEndpoint
import org.unifiedpush.android.connector.data.PushMessage

/**
 * The UnifiedPush connector's events ([UnifiedPushes]): an account's endpoint goes to Rust, which registers it with the
 * account's server; a message, opened by the connector with the account's own keys ([RustKeyManager]), is that
 * account's (the instance) and is shown as an FCM push is once opened ([Push.onOpened]). One the connector could not
 * open is dropped (logged).
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
        val received = System.currentTimeMillis()
        PushCheck.arrived(app, "unifiedpush")
        val account = ChatRules.accountId(instance) ?: return
        if (!message.decrypted) {
            Log.w(TAG, "A UnifiedPush message for $account that did not open is dropped")
            return
        }
        val opened = try {
            JSONObject(String(message.content, Charsets.UTF_8))
        } catch (err: Exception) {
            Log.w(TAG, "A UnifiedPush message for $account is not JSON: dropped")
            return
        }
        opened.put("account", account)
        Push.onOpened(app, opened, received)
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

    private companion object {
        const val TAG = "moqui-push"
    }
}
