// This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License.

package org.moqui.mobile

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.media.AudioAttributes
import android.media.RingtoneManager
import android.net.Uri
import android.os.Build
import android.os.IBinder
import android.util.Log
import androidx.core.app.NotificationCompat
import androidx.core.app.NotificationManagerCompat
import androidx.core.app.Person
import androidx.core.app.ServiceCompat
import androidx.core.content.ContextCompat

/**
 * The `phoneCall` foreground service of the app's calls ([CallManager]): its notification is the ring (high priority,
 * with Answer and Decline and a full-screen intent to [IncomingCallActivity]; a heads-up notification where Android 14+
 * does not let the app use full-screen intents) and then the call in progress, until no call is left and no decline is
 * still being sent. It keeps the process, and the WebView's media, while the screen is off.
 */
class CallService : Service() {
    companion object {
        private const val RING_CHANNEL = "calls-ring"
        private const val CALL_CHANNEL = "calls-ongoing"
        private const val NOTIFICATION_ID = 7301

        /** The id of the notification posted last: each ring has its own, so a ring that follows another is posted
         *  anew, and its full-screen intent fires (an update of a notification fires none). */
        @Volatile private var postedId = NOTIFICATION_ID

        private fun idFor(ringId: String?): Int =
            if (ringId == null) NOTIFICATION_ID else NOTIFICATION_ID + 1 + (ringId.hashCode() and 0xffff)
        const val ACTION_ANSWER = "org.moqui.mobile.ANSWER"
        const val ACTION_DECLINE = "org.moqui.mobile.DECLINE"
        const val ACTION_HANG_UP = "org.moqui.mobile.HANG_UP"

        @Volatile private var running = false

        /** Start, update or stop the service as [CallManager]'s calls are. */
        fun refresh(context: Context) {
            val app = context.applicationContext
            if (CallManager.busy()) {
                try {
                    ContextCompat.startForegroundService(app, Intent(app, CallService::class.java))
                } catch (err: Exception) {
                    // Android 12+ refuses a foreground service started from the background without an exemption (a
                    // high-priority FCM message is one): the ring is a notification of its own then
                    Log.w(CallManager.TAG, "The call service did not start (${err.javaClass.simpleName}): the ring is a plain notification")
                    postPlain(app)
                }
            } else {
                NotificationManagerCompat.from(app).cancel(postedId)
                if (running) app.stopService(Intent(app, CallService::class.java))
            }
        }

        /** The service's notification posted on its own, when the service cannot be a foreground one. */
        fun postPlain(context: Context) {
            channels(context)
            try {
                val id = idFor(CallManager.ringing()?.id)
                val manager = NotificationManagerCompat.from(context)
                manager.notify(id, notification(context, false))
                if (id != postedId) manager.cancel(postedId)
                postedId = id
            } catch (denied: SecurityException) {
                Log.w(CallManager.TAG, "Notifications are off for this app: the ring is not shown")
            } catch (err: Exception) {
                Log.e(CallManager.TAG, "The call notification was not shown", err)
            }
        }

        private fun channels(context: Context) {
            val manager = context.getSystemService(NotificationManager::class.java) ?: return
            if (manager.getNotificationChannel(RING_CHANNEL) == null) {
                manager.createNotificationChannel(
                    NotificationChannel(RING_CHANNEL, "Incoming calls", NotificationManager.IMPORTANCE_HIGH).apply {
                        description = "A call ringing for you"
                        setSound(
                            RingtoneManager.getDefaultUri(RingtoneManager.TYPE_RINGTONE),
                            AudioAttributes.Builder()
                                .setUsage(AudioAttributes.USAGE_NOTIFICATION_RINGTONE)
                                .setContentType(AudioAttributes.CONTENT_TYPE_SONIFICATION)
                                .build()
                        )
                        enableVibration(true)
                        lockscreenVisibility = Notification.VISIBILITY_PUBLIC
                    }
                )
            }
            if (manager.getNotificationChannel(CALL_CHANNEL) == null) {
                manager.createNotificationChannel(
                    NotificationChannel(CALL_CHANNEL, "Calls in progress", NotificationManager.IMPORTANCE_LOW).apply {
                        description = "The call you are in"
                    }
                )
            }
        }

        /** Whether the app may show a full-screen ring (Android 14+ asks the user; earlier always). */
        fun fullScreenAllowed(context: Context): Boolean =
            Build.VERSION.SDK_INT < 34 || NotificationManagerCompat.from(context).canUseFullScreenIntent()

        private fun flags() = PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT

        private fun ringScreen(context: Context, id: String, action: String?): PendingIntent {
            val intent = Intent(context, IncomingCallActivity::class.java)
                .putExtra(CallManager.EXTRA_CALL, id)
                .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_NO_USER_ACTION)
            if (action != null) intent.action = action
            return PendingIntent.getActivity(context, if (action == null) 1 else 2, intent, flags())
        }

        private fun declineAction(context: Context, id: String): PendingIntent =
            PendingIntent.getBroadcast(
                context, 3,
                Intent(context, CallActionReceiver::class.java).setAction(ACTION_DECLINE).putExtra(CallManager.EXTRA_CALL, id),
                flags()
            )

        private fun appLink(context: Context): PendingIntent {
            val intent = Intent(context, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
            return PendingIntent.getActivity(context, 4, intent, flags())
        }

        /** The service's notification: the ring of the first call ringing, else the call in progress. */
        fun notification(context: Context, foreground: Boolean): Notification {
            channels(context)
            val ringing = CallManager.ringing()
            val answered = CallManager.calls().firstOrNull { it.state == CallManager.State.ANSWERED }
            val builder: NotificationCompat.Builder
            if (ringing != null) {
                val caller = Person.Builder().setName(ringing.caller).setImportant(true).build()
                val decline = declineAction(context, ringing.id)
                val answer = ringScreen(context, ringing.id, ACTION_ANSWER)
                val full = ringScreen(context, ringing.id, null)
                builder = NotificationCompat.Builder(context, RING_CHANNEL)
                    .setSmallIcon(R.mipmap.ic_launcher)
                    .setContentTitle(ringing.caller)
                    .setContentText(if (ringing.video) "Incoming video call" else "Incoming call")
                    .setCategory(NotificationCompat.CATEGORY_CALL)
                    .setPriority(NotificationCompat.PRIORITY_MAX)
                    .setVisibility(NotificationCompat.VISIBILITY_PUBLIC)
                    .setOngoing(true)
                    .setContentIntent(full)
                    .setTimeoutAfter((ringing.expiresAt - System.currentTimeMillis()).coerceAtLeast(1_000))
                val fullScreen = fullScreenAllowed(context)
                if (fullScreen) builder.setFullScreenIntent(full, true)
                // CallStyle needs a foreground service or a full-screen intent; else the plain Answer and Decline
                if (foreground || fullScreen) {
                    builder.setStyle(NotificationCompat.CallStyle.forIncomingCall(caller, decline, answer).setIsVideo(ringing.video))
                } else {
                    builder.addAction(0, "Decline", decline).addAction(0, "Answer", answer)
                }
                if (!fullScreen) Log.i(CallManager.TAG, "No full-screen intent for this app (Android 14+): the ring is a heads-up notification")
            } else {
                builder = NotificationCompat.Builder(context, CALL_CHANNEL)
                    .setSmallIcon(R.mipmap.ic_launcher)
                    .setContentTitle(if (answered != null) "Call in progress" else "Declining the call")
                    .setCategory(NotificationCompat.CATEGORY_CALL)
                    .setOngoing(true)
                    .setContentIntent(appLink(context))
                if (answered != null && foreground) {
                    val person = Person.Builder().setName(answered.caller.ifBlank { "MR Moqui" }).build()
                    val hangUp = PendingIntent.getBroadcast(
                        context, 6,
                        Intent(context, CallActionReceiver::class.java).setAction(ACTION_HANG_UP)
                            .putExtra(CallManager.EXTRA_CALL, answered.id),
                        flags()
                    )
                    builder.setStyle(NotificationCompat.CallStyle.forOngoingCall(person, hangUp))
                }
            }
            val notification = builder.build()
            if (ringing != null) notification.flags = notification.flags or Notification.FLAG_INSISTENT
            return notification
        }
    }

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        CallManager.know(this)
        val notification = notification(this, true)
        try {
            val type = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) ServiceInfo.FOREGROUND_SERVICE_TYPE_PHONE_CALL else 0
            val id = idFor(CallManager.ringing()?.id)
            ServiceCompat.startForeground(this, id, notification, type)
            if (id != postedId) NotificationManagerCompat.from(this).cancel(postedId)
            postedId = id
            running = true
        } catch (err: Exception) {
            // the ring is not lost: its notification is posted on its own
            Log.e(CallManager.TAG, "The call service could not be a foreground service", err)
            postPlain(this)
            stopSelf()
            return START_NOT_STICKY
        }
        if (!CallManager.busy()) {
            ServiceCompat.stopForeground(this, ServiceCompat.STOP_FOREGROUND_REMOVE)
            stopSelf()
        }
        return START_NOT_STICKY
    }

    override fun onDestroy() {
        running = false
        super.onDestroy()
    }
}

/** The ring notification's Decline, with no activity. */
class CallActionReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        val id = intent.getStringExtra(CallManager.EXTRA_CALL) ?: return
        when (intent.action) {
            CallService.ACTION_DECLINE -> CallManager.decline(context, id)
            CallService.ACTION_HANG_UP -> CallManager.hangUp(context, id)
        }
    }
}
