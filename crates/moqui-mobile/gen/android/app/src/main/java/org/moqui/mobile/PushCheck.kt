// This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License.

package org.moqui.mobile

import android.annotation.SuppressLint
import android.app.ActivityManager
import android.app.ApplicationExitInfo
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
 * full-screen intent of calls allowed (Android 14+), Firebase in the build, the UnifiedPush distributor, the last push
 * received and the ack ids of the recent ones (the test's own is looked for among them), and when the app was last
 * force-stopped; and the system screen that fixes each ([fix]).
 */
object PushCheck {
    private const val PREFS = "moqui-push"
    /** How many recent ack ids are kept for the check's test. */
    private const val RECENT = 20

    /** A push arrived (before it is opened or shown): the check's "last push received". */
    fun arrived(context: Context, kind: String) {
        context.getSharedPreferences(PREFS, Context.MODE_PRIVATE).edit()
            .putLong("lastPushDate", System.currentTimeMillis())
            .putString("lastPushKind", kind)
            .apply()
    }

    /** A push shown, with its ack id: kept among the recent ones with when it arrived, for the test's round trip. */
    @Synchronized
    fun received(context: Context, kind: String, ackId: String) {
        val prefs = context.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
        val now = System.currentTimeMillis()
        val arrived = prefs.getLong("lastPushDate", now).takeIf { it > 0 } ?: now
        val recent = try {
            JSONObject(prefs.getString("recentAcks", "{}") ?: "{}")
        } catch (err: Exception) {
            JSONObject()
        }
        if (ackId.isNotEmpty()) recent.put(ackId, arrived)
        val kept = JSONObject()
        recent.keys().asSequence().map { it to recent.optLong(it) }.sortedByDescending { it.second }.take(RECENT)
            .forEach { (id, at) -> kept.put(id, at) }
        prefs.edit()
            .putString("lastPushKind", kind)
            .putString("lastAckId", ackId)
            .putString("recentAcks", kept.toString())
            .apply()
    }

    /**
     * When the user last force-stopped the app (Android 11+; 0: not known): a force-stopped app gets no push until it
     * is opened again, and the app says so once ([MainActivity]).
     */
    fun forceStoppedDate(context: Context): Long {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.R) return 0L
        return try {
            val manager = context.getSystemService(ActivityManager::class.java) ?: return 0L
            manager.getHistoricalProcessExitReasons(context.packageName, 0, 5)
                .firstOrNull { it.reason == ApplicationExitInfo.REASON_USER_REQUESTED }?.timestamp ?: 0L
        } catch (err: Exception) {
            0L
        }
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
        val recent = try {
            JSONObject(prefs.getString("recentAcks", "{}") ?: "{}")
        } catch (err: Exception) {
            JSONObject()
        }
        val stopped = forceStoppedDate(context)
        return JSONObject()
            .put("permission", manager.areNotificationsEnabled())
            .put("channels", channels)
            .put("batteryOptimized", power?.isIgnoringBatteryOptimizations(context.packageName) == false)
            .put("fullScreen", fullScreen)
            .put("fcm", BuildConfig.FCM)
            .put("distributor", UnifiedPushes.distributor(context) ?: JSONObject.NULL)
            .put("distributors", UnifiedPushes.distributors(context).size)
            .put("lastPushDate", prefs.getLong("lastPushDate", 0L).takeIf { it > 0 } ?: JSONObject.NULL)
            .put("lastPushKind", prefs.getString("lastPushKind", null) ?: JSONObject.NULL)
            .put("lastAckId", prefs.getString("lastAckId", null) ?: JSONObject.NULL)
            .put("recentAcks", recent)
            .put("forceStoppedDate", if (stopped > 0) stopped else JSONObject.NULL)
            .toString()
    }

    /**
     * Open the system's screen that fixes `what`: `notifications` and `channels` (the app's notification settings),
     * `battery` (the request to leave the app out of battery optimization: one tap), `fullScreen` (Android 14+), and
     * `distributor` (pick a UnifiedPush distributor, where the phone has one), and `firebase` (back from UnifiedPush to
     * Firebase, where the build has it).
     */
    @JvmStatic
    @SuppressLint("BatteryLife")
    fun fix(context: Context, what: String) {
        val intent = when (what) {
            "distributor" -> {
                UnifiedPushes.pick(context)
                return
            }
            "firebase" -> {
                UnifiedPushes.useFirebase(context)
                return
            }
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
