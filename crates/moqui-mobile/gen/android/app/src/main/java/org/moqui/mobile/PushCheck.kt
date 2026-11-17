// This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License.

package org.moqui.mobile

import android.annotation.SuppressLint
import android.app.NotificationManager
import android.content.Context
import android.content.Intent
import android.net.Uri
import android.os.Build
import android.os.PowerManager
import android.provider.Settings
import android.util.Log
import androidx.core.app.NotificationManagerCompat
import org.json.JSONObject

/**
 * The phone's side of the shell's notification check (plans/Comms-Messaging.md §7.6; `push_check` in src/commands.rs,
 * through JNI, src/android.rs): whether notifications are allowed, each channel on, battery optimization off, the
 * full-screen intent of calls allowed (Android 14+), Firebase in the build, and the last push received; and the
 * system screen that fixes each ([fix]).
 */
object PushCheck {
    private const val PREFS = "moqui-push"

    /** A push shown (its kind and ackId), for the check's "last push received" and the test's round trip. */
    fun received(context: Context, kind: String, ackId: String) {
        context.getSharedPreferences(PREFS, Context.MODE_PRIVATE).edit()
            .putLong("lastPushDate", System.currentTimeMillis())
            .putString("lastPushKind", kind)
            .putString("lastAckId", ackId)
            .apply()
    }

    /** The state, as JSON text. */
    @JvmStatic
    fun state(context: Context): String {
        ChatNotifications.ensureChannels(context)
        val manager = NotificationManagerCompat.from(context)
        val channels = JSONObject()
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            val system = context.getSystemService(NotificationManager::class.java)
            for (id in ChatRules.CHANNELS + listOf("calls-ring")) {
                val channel = system?.getNotificationChannel(id) ?: continue
                channels.put(id, channel.importance != NotificationManager.IMPORTANCE_NONE)
            }
        }
        val power = context.getSystemService(PowerManager::class.java)
        val prefs = context.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
        val fullScreen = if (Build.VERSION.SDK_INT >= 34) {
            context.getSystemService(NotificationManager::class.java)?.canUseFullScreenIntent() ?: false
        } else {
            true
        }
        return JSONObject()
            .put("permission", manager.areNotificationsEnabled())
            .put("channels", channels)
            .put("batteryOptimized", power?.isIgnoringBatteryOptimizations(context.packageName) == false)
            .put("fullScreen", fullScreen)
            .put("fcm", BuildConfig.FCM)
            .put("lastPushDate", prefs.getLong("lastPushDate", 0L).takeIf { it > 0 } ?: JSONObject.NULL)
            .put("lastPushKind", prefs.getString("lastPushKind", null) ?: JSONObject.NULL)
            .put("lastAckId", prefs.getString("lastAckId", null) ?: JSONObject.NULL)
            .toString()
    }

    /** Open the system's screen that fixes `what`: `notifications` and `channels` (the app's notification settings),
     * `battery` (the request to leave the app out of battery optimization: one tap), `fullScreen` (Android 14+). */
    @JvmStatic
    @SuppressLint("BatteryLife")
    fun fix(context: Context, what: String) {
        val intent = when (what) {
            "battery" -> Intent(Settings.ACTION_REQUEST_IGNORE_BATTERY_OPTIMIZATIONS, Uri.parse("package:${context.packageName}"))
            "fullScreen" -> if (Build.VERSION.SDK_INT >= 34) {
                Intent(Settings.ACTION_MANAGE_APP_USE_FULL_SCREEN_INTENT, Uri.parse("package:${context.packageName}"))
            } else {
                return
            }
            else -> Intent(Settings.ACTION_APP_NOTIFICATION_SETTINGS).putExtra(Settings.EXTRA_APP_PACKAGE, context.packageName)
        }
        try {
            context.startActivity(intent.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
        } catch (err: Exception) {
            Log.w(ChatNotifications.TAG, "The settings of $what did not open: ${err.message}")
        }
    }
}
