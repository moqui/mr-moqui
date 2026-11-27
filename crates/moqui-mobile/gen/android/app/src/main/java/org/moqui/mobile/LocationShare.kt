// This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License.

package org.moqui.mobile

import android.Manifest
import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.content.pm.ServiceInfo
import android.location.Location
import android.location.LocationListener
import android.location.LocationManager
import android.net.Uri
import android.os.Build
import android.os.Bundle
import android.os.Handler
import android.os.IBinder
import android.os.Looper
import android.provider.Settings
import android.text.format.DateFormat
import android.util.Log
import androidx.core.app.NotificationCompat
import androidx.core.app.NotificationManagerCompat
import androidx.core.content.ContextCompat
import org.json.JSONObject
import java.util.Date
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit

/**
 * Live location sent from this phone (plans/Location-Sharing.md §3.4): a share the person started, or took over with
 * Send from this device, on the Maps page, sent by [LocationShareService], a foreground service of type location, also
 * while the app is in the background, with its notification ("Sharing your location with Sam until 15:30", Stop) for as
 * long as it runs. It is started only from the page's Share while the app is in front ([start], the host command
 * `location_share_start`), so the app has no background location permission. The page asks [allow] first, which gives
 * the reason before Android's question. Each fix goes to Rust (`Native.reportLocation`, the host command
 * `report_location`), which keeps the newest point and sends it with the account's device key; a share stops at its
 * end, on a 410 or 404, when another device sends it, at a sign-out or removal of its account ([forgetAccount]), on Stop
 * and when the location permission is taken away. No position is logged.
 */
object LocationShare {
    internal const val TAG = "moqui-location"
    internal const val CHANNEL = "location-sharing"
    /** Not a call's (7301) nor the screen share's (7302): each shows while it runs. */
    internal const val NOTIFICATION_ID = 7303
    internal const val ACTION_STOP = "org.moqui.mobile.LOCATION_STOP"
    const val ASK_LOCATION = 4
    private const val PREFS = "moqui-location"
    /** The longest [allow] waits for the person's answer. */
    private const val ASK_WAIT_S = 60L

    /** A share sent from here. */
    data class Sent(
        val account: String,
        val shareId: String,
        val thruMs: Long,
        val approximate: Boolean,
        val intervalMs: Long,
        val audience: String,
    ) {
        val key: String get() = "$account/$shareId"
    }

    private val shares = LinkedHashMap<String, Sent>()

    /** The app is in front (MainActivity's resume and pause): the service starts only then. */
    @Volatile var foreground = false

    @Volatile private var answer: CountDownLatch? = null

    @Synchronized
    fun sent(): List<Sent> = shares.values.toList()

    @Synchronized
    internal fun remove(key: String): Boolean = shares.remove(key) != null

    /** Whether the app may read the location: precise for `exact`, at least approximate otherwise. */
    fun permitted(context: Context, exact: Boolean = false): Boolean {
        val fine = context.checkSelfPermission(Manifest.permission.ACCESS_FINE_LOCATION) == PackageManager.PERMISSION_GRANTED
        val coarse = context.checkSelfPermission(Manifest.permission.ACCESS_COARSE_LOCATION) == PackageManager.PERMISSION_GRANTED
        return if (exact) fine else fine || coarse
    }

    private fun notificationsShown(context: Context): Boolean {
        if (!NotificationManagerCompat.from(context).areNotificationsEnabled()) return false
        val channel = context.getSystemService(NotificationManager::class.java)?.getNotificationChannel(CHANNEL)
        return channel == null || channel.importance != NotificationManager.IMPORTANCE_NONE
    }

    /**
     * Before the first share from this phone: the location permission, asked after the app's dialog that says why
     * (precise and approximate both asked: Android lets the person choose either; an exact share with only approximate
     * given sends approximate fixes). Whether the app may read the location now. Waits for the answer, so never on the
     * main thread.
     */
    @JvmStatic
    fun allow(context: Context, exact: Boolean): Boolean {
        val app = context.applicationContext
        if (permitted(app, false)) return true
        val activity = CallManager.appActivity() ?: return false
        app.getSharedPreferences(PREFS, Context.MODE_PRIVATE).edit().putBoolean("asked", true).apply()
        val waiting = CountDownLatch(1)
        answer = waiting
        Handler(Looper.getMainLooper()).post {
            if (activity.isFinishing || activity.isDestroyed) {
                waiting.countDown()
                return@post
            }
            var asked = false
            ObscuredTouches.show(
                android.app.AlertDialog.Builder(activity)
                    .setTitle(R.string.ask_location_title)
                    .setMessage(if (exact) R.string.ask_location_reason else R.string.ask_location_reason_approximate)
                    .setPositiveButton(R.string.ask_continue) { _, _ ->
                        asked = true
                        activity.requestPermissions(
                            arrayOf(Manifest.permission.ACCESS_FINE_LOCATION, Manifest.permission.ACCESS_COARSE_LOCATION),
                            ASK_LOCATION
                        )
                    }
                    .setNegativeButton(R.string.ask_not_now, null)
                    .setOnDismissListener { if (!asked) waiting.countDown() }
            )
        }
        waiting.await(ASK_WAIT_S, TimeUnit.SECONDS)
        answer = null
        return permitted(app, false)
    }

    /** Android's question was answered ([MainActivity.onRequestPermissionsResult]). */
    fun answered() {
        answer?.countDown()
    }

    /**
     * The page's Share (`location_share_start`): the share in `json` ({account, shareId, thruDate, approximate,
     * intervalSeconds, audience}) sent from here by the service from now on. Only with the app in front, the location
     * permission and notifications allowed (the notification is what shows it is sent); whether it started.
     */
    @JvmStatic
    fun start(context: Context, json: String): Boolean {
        val app = context.applicationContext
        val share = try {
            val o = JSONObject(json)
            Sent(
                o.getString("account"), o.getString("shareId"), o.getLong("thruDate"), o.optBoolean("approximate"),
                o.getLong("intervalSeconds") * 1000L, o.optString("audience")
            )
        } catch (err: Exception) {
            Log.w(TAG, "Not a share to send: ${err.message}")
            return false
        }
        ensureChannel(app)
        if (!LocationRules.mayStart(foreground, permitted(app), notificationsShown(app))) {
            Log.i(TAG, "Share ${share.shareId} is not sent by the service: the app is not in front, or location or notifications are off")
            return false
        }
        synchronized(this) { shares[share.key] = share }
        return try {
            ContextCompat.startForegroundService(app, Intent(app, LocationShareService::class.java))
            true
        } catch (err: Exception) {
            Log.w(TAG, "The location service did not start: ${err.message}")
            remove(share.key)
            false
        }
    }

    /** The page stopped a share, or it is sent elsewhere now (`location_share_stop`): no longer sent from here. */
    @JvmStatic
    fun stop(context: Context, account: String, shareId: String) {
        if (remove("$account/$shareId")) changed(context.applicationContext)
    }

    /** `account` is signed out or removed: none of its shares is sent from here. */
    @JvmStatic
    fun forgetAccount(context: Context, account: String) {
        val gone = synchronized(this) {
            val keys = shares.keys.filter { it.startsWith("$account/") }
            keys.forEach { shares.remove(it) }
            keys.isNotEmpty()
        }
        if (gone) changed(context.applicationContext)
    }

    /** The shares changed: the service updates its notification and what it asks of Android, or ends with none. */
    internal fun changed(context: Context) {
        try {
            context.startService(Intent(context, LocationShareService::class.java).setAction(LocationShareService.ACTION_CHANGED))
        } catch (err: Exception) {
            // not running, or not allowed to start from here: nothing to change
            Log.d(TAG, "The location service was not told of a change: ${err.message}")
        }
    }

    /** The notification check's items: the permission (`fine`, `coarse` or `none`) and how many shares are sent now. */
    fun state(context: Context): JSONObject {
        val fine = permitted(context, true)
        return JSONObject()
            .put("permission", if (fine) "fine" else if (permitted(context)) "coarse" else "none")
            .put("asked", context.getSharedPreferences(PREFS, Context.MODE_PRIVATE).getBoolean("asked", false))
            .put("sharing", sent().size)
    }

    /** The check's fix: the app's settings, where the person allows the location. */
    fun fix(context: Context) {
        try {
            context.startActivity(
                Intent(Settings.ACTION_APPLICATION_DETAILS_SETTINGS, Uri.parse("package:${context.packageName}"))
                    .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
            )
        } catch (err: Exception) {
            Log.w(TAG, "The app's settings did not open: ${err.message}")
        }
    }

    internal fun ensureChannel(context: Context) {
        val manager = context.getSystemService(NotificationManager::class.java) ?: return
        if (manager.getNotificationChannel(CHANNEL) == null) {
            manager.createNotificationChannel(
                NotificationChannel(CHANNEL, context.getString(R.string.channel_location), NotificationManager.IMPORTANCE_LOW).apply {
                    description = context.getString(R.string.channel_location_about)
                }
            )
        }
    }

    /** The permanent notification while shares are sent: whom with and until when, with Stop. */
    internal fun notification(context: Context, sent: List<Sent>): Notification {
        ensureChannel(context)
        val stop = PendingIntent.getService(
            context, 0, Intent(context, LocationShareService::class.java).setAction(ACTION_STOP),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT
        )
        val open = context.packageManager.getLaunchIntentForPackage(context.packageName)?.let {
            PendingIntent.getActivity(context, 0, it, PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT)
        }
        val last = sent.maxOfOrNull { it.thruMs } ?: System.currentTimeMillis()
        val until = DateFormat.getTimeFormat(context).format(Date(last))
        val text = LocationRules.notificationText(sent.map { it.audience }, until)
        return NotificationCompat.Builder(context, CHANNEL)
            .setSmallIcon(android.R.drawable.ic_menu_mylocation)
            .setContentTitle(context.getString(R.string.location_sharing_title))
            .setContentText(text)
            .setStyle(NotificationCompat.BigTextStyle().bigText(text))
            .setOngoing(true)
            .setOnlyAlertOnce(true)
            .setCategory(NotificationCompat.CATEGORY_SERVICE)
            .setForegroundServiceBehavior(NotificationCompat.FOREGROUND_SERVICE_IMMEDIATE)
            .apply { if (open != null) setContentIntent(open) }
            .addAction(0, context.getString(R.string.location_stop), stop)
            .build()
    }
}

/**
 * The foreground service of type location that sends the shares of [LocationShare] (plans/Location-Sharing.md §3.4):
 * Android's LocationManager (no Google Play services), at the interval [LocationRules] chooses from the shares (a few
 * seconds moving, longer still, an approximate share's own), each fix handed to Rust on the native code's thread and
 * what it came to read back: a stopped share goes, a point that waits is sent again when its wait is over. A check each
 * half minute ends a share at its end and every share when the permission is gone. Started only by [LocationShare.start];
 * not sticky, so a process Android ended does not start it again in the background.
 */
class LocationShareService : Service(), LocationListener {
    companion object {
        const val ACTION_CHANGED = "org.moqui.mobile.LOCATION_CHANGED"
        private const val CHECK_MS = 30_000L
    }

    private val main = Handler(Looper.getMainLooper())
    private var manager: LocationManager? = null
    private var asked: Pair<String, Long>? = null
    private var moving = true
    private var last: Location? = null
    private val waiting = HashSet<String>()
    private val check = object : Runnable {
        override fun run() {
            checkShares()
            main.postDelayed(this, CHECK_MS)
        }
    }

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        val sent = LocationShare.sent()
        // in the foreground at once, as Android asks of a service started for the foreground, even to end
        val notification = LocationShare.notification(this, sent)
        try {
            if (Build.VERSION.SDK_INT >= 29) {
                startForeground(LocationShare.NOTIFICATION_ID, notification, ServiceInfo.FOREGROUND_SERVICE_TYPE_LOCATION)
            } else {
                startForeground(LocationShare.NOTIFICATION_ID, notification)
            }
        } catch (err: Exception) {
            // Android refused (not started from the app in front, or no permission): nothing is sent
            Log.w(LocationShare.TAG, "The location service could not run: ${err.message}")
            sent.forEach { LocationShare.remove(it.key) }
            end()
            return START_NOT_STICKY
        }
        if (intent?.action == LocationShare.ACTION_STOP) {
            // the person's Stop: each share stopped on its server too, so its audience is told at once
            val dir = Native.dataDir(this)
            for (share in sent) {
                LocationShare.remove(share.key)
                Native.work { Native.stopLocation(dir, share.account, share.shareId) }
            }
            Log.i(LocationShare.TAG, "Stop: ${sent.size} shares are no longer sent")
            end()
            return START_NOT_STICKY
        }
        if (sent.isEmpty()) {
            end()
            return START_NOT_STICKY
        }
        getSystemService(NotificationManager::class.java)?.notify(LocationShare.NOTIFICATION_ID, notification)
        listen()
        main.removeCallbacks(check)
        main.postDelayed(check, CHECK_MS)
        return START_NOT_STICKY
    }

    /** LocationManager asked for fixes at the shares' interval, from the provider the rules choose. */
    private fun listen() {
        val lm = manager ?: getSystemService(LocationManager::class.java).also { manager = it } ?: return
        val sent = LocationShare.sent()
        val exact = sent.any { !it.approximate } && LocationShare.permitted(this, true)
        val provider = LocationRules.provider(
            exact,
            Build.VERSION.SDK_INT >= 31 && lm.allProviders.contains(LocationManager.FUSED_PROVIDER),
            lm.isProviderEnabled(LocationManager.GPS_PROVIDER),
            lm.isProviderEnabled(LocationManager.NETWORK_PROVIDER)
        ) ?: run {
            Log.i(LocationShare.TAG, "No location provider is on: the shares wait for one")
            return
        }
        val interval = LocationRules.serviceIntervalMs(sent.map { it.approximate to it.intervalMs }, moving)
        if (asked == provider to interval) return
        try {
            lm.removeUpdates(this)
            lm.requestLocationUpdates(provider, interval, 0f, this, Looper.getMainLooper())
            asked = provider to interval
        } catch (err: SecurityException) {
            Log.i(LocationShare.TAG, "The location permission is gone: the shares stop")
            stopAll()
        }
    }

    override fun onLocationChanged(location: Location) {
        val before = last
        last = location
        val nowMoving = LocationRules.moving(
            if (location.hasSpeed()) location.speed.toDouble() else null,
            before?.let { LocationRules.metres(it.latitude, it.longitude, location.latitude, location.longitude) }
        )
        if (nowMoving != moving) {
            moving = nowMoving
            listen()
        }
        val fix = JSONObject()
            .put("latitude", location.latitude)
            .put("longitude", location.longitude)
            .put("time", location.time)
        if (location.hasAccuracy()) fix.put("accuracy", location.accuracy.toDouble())
        if (location.hasBearing()) fix.put("heading", location.bearing.toDouble())
        if (location.hasSpeed()) fix.put("speed", location.speed.toDouble())
        for (share in LocationShare.sent()) report(share, fix.toString())
    }

    @Deprecated("Android's own, before API 29")
    override fun onStatusChanged(provider: String?, status: Int, extras: Bundle?) {}

    override fun onProviderEnabled(provider: String) {
        asked = null
        listen()
    }

    override fun onProviderDisabled(provider: String) {
        asked = null
        listen()
    }

    /** A fix (empty: a point that waited) of `share` to Rust, on the native code's thread; what it came to, read here. */
    private fun report(share: LocationShare.Sent, fix: String) {
        val dir = Native.dataDir(this)
        val app = applicationContext
        Native.work {
            Native.ensure(app)
            val outcome = try {
                JSONObject(Native.reportLocation(dir, share.account, share.shareId, fix) ?: "{}")
            } catch (err: Exception) {
                JSONObject()
            }
            main.post { took(share, outcome) }
        }
    }

    private fun took(share: LocationShare.Sent, outcome: JSONObject) {
        val state = outcome.optString("state")
        val reason = LocationRules.stopReason(share.thruMs, System.currentTimeMillis(), true, true, state, outcome.optString("reason"))
        if (reason != null) {
            Log.i(LocationShare.TAG, "Share ${share.shareId} is no longer sent from here: $reason")
            if (LocationShare.remove(share.key)) changedHere()
            return
        }
        // a point waits (the interval, a 429, no network): sent again when the wait is over, once a share
        if (state == "waiting" && waiting.add(share.key)) {
            main.postDelayed({
                waiting.remove(share.key)
                if (LocationShare.sent().any { it.key == share.key }) report(share, "")
            }, outcome.optLong("waitMs", 5_000L).coerceIn(0L, 3_600_000L))
        }
    }

    /** Each share at its end, and all of them once the permission is gone. */
    private fun checkShares() {
        val now = System.currentTimeMillis()
        val permitted = LocationShare.permitted(this)
        var gone = false
        for (share in LocationShare.sent()) {
            val reason = LocationRules.stopReason(share.thruMs, now, permitted, true, null, null) ?: continue
            Log.i(LocationShare.TAG, "Share ${share.shareId} is no longer sent from here: $reason")
            gone = LocationShare.remove(share.key) || gone
        }
        if (gone) changedHere()
    }

    private fun stopAll() {
        LocationShare.sent().forEach { LocationShare.remove(it.key) }
        end()
    }

    /** The shares changed here: the notification again, or the end with none left. */
    private fun changedHere() {
        val sent = LocationShare.sent()
        if (sent.isEmpty()) {
            end()
            return
        }
        getSystemService(NotificationManager::class.java)?.notify(LocationShare.NOTIFICATION_ID, LocationShare.notification(this, sent))
        asked = null
        listen()
    }

    private fun end() {
        main.removeCallbacksAndMessages(null)
        try {
            manager?.removeUpdates(this)
        } catch (_: Exception) {
        }
        asked = null
        stopForeground(STOP_FOREGROUND_REMOVE)
        stopSelf()
        getSystemService(NotificationManager::class.java)?.cancel(LocationShare.NOTIFICATION_ID)
    }

    override fun onDestroy() {
        main.removeCallbacksAndMessages(null)
        try {
            manager?.removeUpdates(this)
        } catch (_: Exception) {
        }
        getSystemService(NotificationManager::class.java)?.cancel(LocationShare.NOTIFICATION_ID)
        super.onDestroy()
    }
}
