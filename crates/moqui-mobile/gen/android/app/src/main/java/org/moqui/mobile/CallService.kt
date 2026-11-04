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
        const val ACTION_ANSWER = "org.moqui.mobile.ANSWER"
        const val ACTION_DECLINE = "org.moqui.mobile.DECLINE"

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
                    channels(app)
                    try {
                        NotificationManagerCompat.from(app).notify(NOTIFICATION_ID, notification(app, false))
                    } catch (denied: SecurityException) {
                        Log.w(CallManager.TAG, "Notifications are off for this app: the ring is not shown")
                    }
                }
            } else {
                NotificationManagerCompat.from(app).cancel(NOTIFICATION_ID)
                if (running) app.stopService(Intent(app, CallService::class.java))
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

        private fun appLink(context: Context, link: String?): PendingIntent {
            val intent = Intent(context, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
            if (link != null) intent.setAction(Intent.ACTION_VIEW).setData(Uri.parse(link))
            return PendingIntent.getActivity(context, if (link == null) 4 else 5, intent, flags())
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
                    .setContentIntent(appLink(context, null))
                if (answered != null && foreground) {
                    val person = Person.Builder().setName(answered.caller.ifBlank { "MR Moqui" }).build()
                    val hangUp = appLink(context, "org.moqui.mobile:/call?callSessionId=${answered.id}&hangUp=true")
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
            ServiceCompat.startForeground(this, NOTIFICATION_ID, notification, type)
            running = true
        } catch (err: Exception) {
            Log.e(CallManager.TAG, "The call service could not be a foreground service", err)
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
        if (intent.action == CallService.ACTION_DECLINE) CallManager.decline(context, id)
    }
}
