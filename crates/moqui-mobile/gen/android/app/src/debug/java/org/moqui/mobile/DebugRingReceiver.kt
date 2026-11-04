// This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License.

package org.moqui.mobile

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.util.Log

/**
 * Debug builds only (src/debug): a push injected by adb, through the same handler as FCM's, for a check without a
 * Firebase project. Only the shell (which holds android.permission.DUMP) may send it:
 *
 *   adb shell cmd deviceidle tempwhitelist -d 60000 org.moqui.mobile   # as a high-priority FCM message allows
 *   adb shell am broadcast -n org.moqui.mobile/.DebugRingReceiver --es kind ring --es callSessionId 100100 \
 *       --es caller 'John Doe' --es video false --el ringSeconds 40
 *   adb shell am broadcast -n org.moqui.mobile/.DebugRingReceiver --es kind ringEnded --es callSessionId 100100 --es reason timedOut
 *   adb shell am broadcast -n org.moqui.mobile/.DebugRingReceiver --es kind token --es token debug-token-1
 *
 * `ringSeconds` makes `expiresAt` from now; `expiresAt` (Unix milliseconds) may be given instead.
 */
class DebugRingReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        val extras = intent.extras ?: return
        val data = HashMap<String, String>()
        for (key in extras.keySet()) {
            @Suppress("DEPRECATION")
            extras.get(key)?.let { data[key] = it.toString() }
        }
        data.remove("ringSeconds")?.toLongOrNull()?.let { data["expiresAt"] = (System.currentTimeMillis() + it * 1000).toString() }
        Log.i("moqui-push", "A debug push of kind ${data["kind"]}")
        if (data["kind"] == "token") {
            data["token"]?.let { Push.token(context, it) }
        } else {
            CallManager.onPush(context, data)
        }
    }
}
