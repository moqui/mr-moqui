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
import android.location.LocationRequest
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
import org.json.JSONArray
import org.json.JSONObject
import java.util.Date
import java.util.concurrent.CountDownLatch
import java.util.concurrent.Executors
import java.util.concurrent.TimeUnit

/**
 * Live location sent from this phone (plans/Location-Sharing.md §3.4): a share the person started, or took over with
 * Send from this device, on the Maps page, sent by [LocationShareService], a foreground service of type location, also
 * while the app is in the background, with its notification ("Sharing your location with Sam until 15:30", Stop) for as
 * long as it runs. It is started only from the page's Share while the app is in front ([start], the host command
 * `location_share_start`), so the app has no background location permission, and only once the person said Send in the
 * app's own dialog, whose words (whom with, until when) are the server's: that dialog, not the page, is the control. The page asks [allow] first, which gives
 * the reason before Android's question. Each fix goes to Rust (`Native.reportLocation`, the host command
 * `report_location`) on this object's own thread ([work]), never the native code's shared one, which pushes and a
 * call's hang-up use; Rust keeps the newest point and sends it with the account's device key within 15 seconds. A share
 * stops at its end, on a 410 or 404, when another device sends it, at a sign-out or removal of its account
 * ([forgetAccount]), on Stop, when notifications are turned off and when the location permission is taken away; Stop
 * and notifications off stop it on the server too, durably ([LocationStops]). No position is logged.
 */
object LocationShare {
    internal const val TAG = "moqui-location"
    internal const val CHANNEL = "location-sharing"
    /** Not a call's (7301) nor the screen share's (7302): each shows while it runs. */
    internal const val NOTIFICATION_ID = 7303
    internal const val ACTION_STOP = "org.moqui.mobile.LOCATION_STOP"
    /** The notification was dismissed (Android 14+ lets the person): it is shown again while the shares are sent. */
    internal const val ACTION_REPOST = "org.moqui.mobile.LOCATION_REPOST"
    const val ASK_LOCATION = 4
    private const val PREFS = "moqui-location"
    /** The longest [allow] waits for the person's answer. */
    private const val ASK_WAIT_S = 60L

    /** The reports and stops of the shares, on a thread of their own: a slow server holds up nothing else. */
    private val worker = Executors.newSingleThreadExecutor { task -> Thread(task, "moqui-location") }

    /** A share sent from here. */
    data class Sent(
        val account: String,
        val shareId: String,
        val thruMs: Long,
        val approximate: Boolean,
        val intervalMs: Long,
        val audience: String,
    ) {
        val key: String get() = LocationRules.stopKey(account, shareId)
    }

    private val shares = LinkedHashMap<String, Sent>()

    /** The app is in front (MainActivity's resume and pause): the service starts only then. */
    @Volatile var foreground = false

    @Volatile private var answer: CountDownLatch? = null

    @Synchronized
    fun sent(): List<Sent> = shares.values.toList()

    /**
     * A share no longer sent from here: Rust's reporter goes with it (`Native.forgetLocation`), so the page's status
     * and Rust agree. Whether it was sent.
     */
    internal fun remove(key: String): Boolean {
        val gone = synchronized(this) { shares.remove(key) } ?: return false
        try {
            Native.forgetLocation(gone.account, gone.shareId)
        } catch (err: Throwable) {
            Log.w(TAG, "Rust was not told share ${gone.shareId} is no longer sent: ${err.message}")
        }
        return true
    }

    /** The share sent from here by its key, as it is now; null once it is no longer sent. */
    @Synchronized
    fun current(key: String): Sent? = shares[key]

    /** The shares of `account` sent from here now, a JSON list of ids (`location_share_status`). */
    @JvmStatic
    fun sending(account: String): String = JSONArray(sent().filter { it.account == account }.map { it.shareId }).toString()

    /** `task` on the location thread; a failure is logged. */
    internal fun work(task: () -> Unit) {
        worker.execute {
            try {
                task()
            } catch (err: Throwable) {
                Log.e(TAG, "Location work failed", err)
            }
        }
    }

    fun fine(context: Context): Boolean =
        context.checkSelfPermission(Manifest.permission.ACCESS_FINE_LOCATION) == PackageManager.PERMISSION_GRANTED

    fun coarse(context: Context): Boolean =
        context.checkSelfPermission(Manifest.permission.ACCESS_COARSE_LOCATION) == PackageManager.PERMISSION_GRANTED

    /** Whether the app may read the location: precise for `exact`, at least approximate otherwise. */
    fun permitted(context: Context, exact: Boolean = false): Boolean =
        if (exact) fine(context) else fine(context) || coarse(context)

    internal fun notificationsShown(context: Context): Boolean {
        if (!NotificationManagerCompat.from(context).areNotificationsEnabled()) return false
        val channel = context.getSystemService(NotificationManager::class.java)?.getNotificationChannel(CHANNEL)
        return channel == null || channel.importance != NotificationManager.IMPORTANCE_NONE
    }

    /**
     * Before a share from this phone: the location permission, asked after the app's dialog that says why. An exact
     * share asks for precise and approximate (the person may give either; given approximate only, it sends the phone's
     * approximate fixes), once; an approximate share asks for approximate alone, so where precise was never given the
     * phone itself gives approximate fixes (about 3 km² on Android 12+). Whether the app may read the location now: false
     * on Not now or a dismissed dialog. Waits for the answer, so never on the main thread.
     */
    @JvmStatic
    fun allow(context: Context, exact: Boolean): Boolean {
        val app = context.applicationContext
        val prefs = app.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
        val wanted = LocationRules.permissionsToAsk(exact, fine(app), coarse(app), prefs.getBoolean("askedFine", false))
        if (wanted.isEmpty()) return permitted(app)
        val activity = CallManager.appActivity() ?: return false
        val names = wanted.map { if (it == "fine") Manifest.permission.ACCESS_FINE_LOCATION else Manifest.permission.ACCESS_COARSE_LOCATION }
        val waiting = CountDownLatch(1)
        answer = waiting
        var asked = false
        Handler(Looper.getMainLooper()).post {
            if (activity.isFinishing || activity.isDestroyed) {
                waiting.countDown()
                return@post
            }
            ObscuredTouches.show(
                android.app.AlertDialog.Builder(activity)
                    .setTitle(R.string.ask_location_title)
                    .setMessage(if (exact) R.string.ask_location_reason else R.string.ask_location_reason_approximate)
                    .setPositiveButton(R.string.ask_continue) { _, _ ->
                        asked = true
                        // asked only now that Android's question is: a Not now asks again at the next share
                        prefs.edit().putBoolean("asked", true).apply { if ("fine" in wanted) putBoolean("askedFine", true) }.apply()
                        activity.requestPermissions(names.toTypedArray(), ASK_LOCATION)
                    }
                    .setNegativeButton(R.string.ask_not_now, null)
                    .setOnDismissListener { if (!asked) waiting.countDown() }
            )
        }
        waiting.await(ASK_WAIT_S, TimeUnit.SECONDS)
        answer = null
        // Not now, or the dialog dismissed: no share, whatever was given before
        return LocationRules.allowed(asked, permitted(app))
    }

    /** Android's question was answered ([MainActivity.onRequestPermissionsResult]). */
    fun answered() {
        answer?.countDown()
    }

    /**
     * The page's Share (`location_share_start`): the share in `json` ({account, shareId, thruDate, approximate,
     * intervalSeconds, audience}, all of it the server's: Rust read it with the account's device key, `get#AppShare`)
     * sent from here by the service from now on. Only with the app in front, the location permission and notifications
     * allowed (the notification is what shows it is sent), and once the person said Send in [confirm]: `started`,
     * `declined` (Not now) or `unavailable`. Waits for the answer, so never on the main thread.
     */
    @JvmStatic
    fun start(context: Context, json: String): String {
        val app = context.applicationContext
        val share = try {
            val o = JSONObject(json)
            Sent(
                o.getString("account"), o.getString("shareId"), o.getLong("thruDate"), o.optBoolean("approximate"),
                o.getLong("intervalSeconds") * 1000L, o.optString("audience")
            )
        } catch (err: Exception) {
            Log.w(TAG, "Not a share to send: ${err.message}")
            return "unavailable"
        }
        ensureChannel(app)
        if (!LocationRules.mayStart(foreground, permitted(app), notificationsShown(app))) {
            Log.i(TAG, "Share ${share.shareId} is not sent by the service: the app is not in front, or location or notifications are off")
            return "unavailable"
        }
        if (!confirm(app, share)) {
            Log.i(TAG, "Share ${share.shareId} is not sent from this phone: the person said Not now")
            return "declined"
        }
        synchronized(this) { shares[share.key] = share }
        return try {
            ContextCompat.startForegroundService(app, Intent(app, LocationShareService::class.java))
            "started"
        } catch (err: Exception) {
            Log.w(TAG, "The location service did not start: ${err.message}")
            remove(share.key)
            "unavailable"
        }
    }

    /**
     * The app's own question before a share is sent from this phone: "Send your location from this phone to Sam until
     * 15:30?", Send or Not now, in a native dialog that refuses touches through another app's window
     * ([ObscuredTouches]), with the server's words. A page's script can ask the host to start a share; only the person's
     * Send here starts one. Whether they said Send (no activity, or no answer in a minute: no).
     */
    private fun confirm(context: Context, share: Sent): Boolean {
        val activity = CallManager.appActivity() ?: return false
        val until = DateFormat.getTimeFormat(context).format(Date(share.thruMs))
        val text = LocationRules.confirmText(share.audience, until, share.approximate)
        val waiting = CountDownLatch(1)
        var send = false
        Handler(Looper.getMainLooper()).post {
            if (activity.isFinishing || activity.isDestroyed) {
                waiting.countDown()
                return@post
            }
            ObscuredTouches.show(
                android.app.AlertDialog.Builder(activity)
                    .setTitle(R.string.location_confirm_title)
                    .setMessage(text)
                    .setPositiveButton(R.string.location_confirm_send) { _, _ -> send = true }
                    .setNegativeButton(R.string.ask_not_now, null)
                    .setOnDismissListener { waiting.countDown() }
            )
        }
        waiting.await(ASK_WAIT_S, TimeUnit.SECONDS)
        return LocationRules.confirmed(send, foreground)
    }

    /** The page stopped a share, or it is sent elsewhere now (`location_share_stop`): no longer sent from here. */
    @JvmStatic
    fun stop(context: Context, account: String, shareId: String) {
        if (remove(LocationRules.stopKey(account, shareId))) changed(context.applicationContext)
    }

    /** `account` is signed out or removed: none of its shares is sent from here. */
    @JvmStatic
    fun forgetAccount(context: Context, account: String) {
        val keys = sent().filter { it.account == account }.map { it.key }
        keys.forEach { remove(it) }
        if (keys.isNotEmpty()) changed(context.applicationContext)
    }

    /**
     * The person's Stop, notifications turned off, or the permission gone while the process lives on: every share sent
     * from here is no longer sent, and is stopped on its server, durably ([LocationStops]: tried until the server confirms, with a "Stopping" notification until then).
     */
    internal fun stopAll(context: Context) {
        val sent = sent()
        for (share in sent) {
            remove(share.key)
            LocationStops.add(context, share.account, share.shareId, share.thruMs)
        }
        if (sent.isNotEmpty()) Log.i(TAG, "Stop: ${sent.size} shares are no longer sent, and are stopped on their servers")
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
        return JSONObject()
            .put("permission", if (fine(context)) "fine" else if (coarse(context)) "coarse" else "none")
            .put("asked", context.getSharedPreferences(PREFS, Context.MODE_PRIVATE).getBoolean("asked", false))
            .put("sharing", sent().size)
            .put("stopping", LocationStops.pending(context).size)
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

    private fun serviceIntent(context: Context, action: String, code: Int): PendingIntent = PendingIntent.getService(
        context, code, Intent(context, LocationShareService::class.java).setAction(action),
        PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT
    )

    /**
     * The permanent notification while shares are sent: whom with and until when, with Stop. Dismissing it (Android
     * 14+ lets the person) posts it again (its delete intent), so it shows as long as the service sends.
     */
    internal fun notification(context: Context, sent: List<Sent>): Notification {
        ensureChannel(context)
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
            .setDeleteIntent(serviceIntent(context, ACTION_REPOST, 1))
            .addAction(0, context.getString(R.string.location_stop), serviceIntent(context, ACTION_STOP, 0))
            .build()
    }
}

/**
 * The foreground service of type location that sends the shares of [LocationShare] (plans/Location-Sharing.md §3.4):
 * Android's LocationManager (no Google Play services), at the interval and accuracy [LocationRules] choose from the
 * shares (a few seconds moving, longer still, an approximate share's own interval at a low-power, approximate
 * accuracy), each fix handed to Rust on the location thread, at most one report a share queued or under way, and what
 * it came to read back: a stopped share goes, a point that waits is sent again when its wait is over. A check each half
 * minute ends a share at its end, every share when the permission is gone, and every share (stopped on its server) when
 * notifications are off, else posts the notification again. Started only by [LocationShare.start]; not sticky, so a
 * process Android ended does not start it again in the background.
 */
class LocationShareService : Service(), LocationListener {
    companion object {
        const val ACTION_CHANGED = "org.moqui.mobile.LOCATION_CHANGED"
        private const val CHECK_MS = 30_000L
    }

    private val main = Handler(Looper.getMainLooper())
    private var manager: LocationManager? = null
    private var asked: Triple<String, Long, String>? = null
    private var moving = true
    private var last: Location? = null
    private var inForeground = false
    private val waiting = HashSet<String>()
    /** The shares with a report under way, and the newest fix that came meanwhile for each (one queued at most). */
    private val inFlight = HashSet<String>()
    private val queued = HashMap<String, String>()
    private val check = object : Runnable {
        override fun run() {
            checkShares()
            main.postDelayed(this, CHECK_MS)
        }
    }

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        // the person's Stop first, before anything that could fail: each share stopped on its server, durably
        if (intent?.action == LocationShare.ACTION_STOP) {
            LocationShare.stopAll(this)
            end()
            return START_NOT_STICKY
        }
        val sent = LocationShare.sent()
        if (!inForeground) {
            // in the foreground at once, as Android asks of a service started for the foreground, even to end
            try {
                val notification = LocationShare.notification(this, sent)
                if (Build.VERSION.SDK_INT >= 29) {
                    startForeground(LocationShare.NOTIFICATION_ID, notification, ServiceInfo.FOREGROUND_SERVICE_TYPE_LOCATION)
                } else {
                    startForeground(LocationShare.NOTIFICATION_ID, notification)
                }
                inForeground = true
            } catch (err: Exception) {
                // Android refused (not started from the app in front, or no permission): nothing is sent from here
                Log.w(LocationShare.TAG, "The location service could not run: ${err.message}")
                sent.forEach { LocationShare.remove(it.key) }
                end()
                return START_NOT_STICKY
            }
        }
        if (sent.isEmpty()) {
            end()
            return START_NOT_STICKY
        }
        // a change, a dismissed notification (ACTION_REPOST) or a new share: the notification again, and the fixes
        post(sent)
        listen()
        main.removeCallbacks(check)
        main.postDelayed(check, CHECK_MS)
        return START_NOT_STICKY
    }

    private fun post(sent: List<LocationShare.Sent>) {
        getSystemService(NotificationManager::class.java)?.notify(LocationShare.NOTIFICATION_ID, LocationShare.notification(this, sent))
    }

    /** LocationManager asked for fixes at the shares' interval and accuracy, from the provider the rules choose. */
    private fun listen() {
        val lm = manager ?: getSystemService(LocationManager::class.java).also { manager = it } ?: return
        val sent = LocationShare.sent()
        val exact = sent.any { !it.approximate } && LocationShare.permitted(this, true)
        val provider = LocationRules.provider(
            exact,
            LocationShare.fine(this),
            Build.VERSION.SDK_INT >= 31 && lm.allProviders.contains(LocationManager.FUSED_PROVIDER),
            lm.isProviderEnabled(LocationManager.GPS_PROVIDER),
            lm.isProviderEnabled(LocationManager.NETWORK_PROVIDER)
        ) ?: run {
            Log.i(LocationShare.TAG, "No location provider is on: the shares wait for one")
            return
        }
        val interval = LocationRules.serviceIntervalMs(sent.map { it.approximate to it.intervalMs }, moving)
        val quality = LocationRules.quality(exact)
        if (asked == Triple(provider, interval, quality)) return
        try {
            lm.removeUpdates(this)
            if (Build.VERSION.SDK_INT >= 31) {
                val request = LocationRequest.Builder(interval)
                    .setQuality(if (quality == "high") LocationRequest.QUALITY_HIGH_ACCURACY else LocationRequest.QUALITY_BALANCED_POWER_ACCURACY)
                    .build()
                lm.requestLocationUpdates(provider, request, ContextCompat.getMainExecutor(this), this)
            } else {
                lm.requestLocationUpdates(provider, interval, 0f, this, Looper.getMainLooper())
            }
            asked = Triple(provider, interval, quality)
        } catch (err: SecurityException) {
            // a provider this permission does not reach: wait for one (a provider turned on), as with none; the
            // half-minute check stops the shares once the permission is gone
            Log.i(LocationShare.TAG, "The location provider $provider is not allowed: the shares wait for one")
            asked = null
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

    /**
     * A fix (empty: a point that waited) of `share` to Rust on the location thread, what it came to read here. While a
     * report of the share is under way the newest fix waits ([LocationRules.queueFix]), so reports never pile up.
     */
    private fun report(share: LocationShare.Sent, fix: String) {
        val key = share.key
        if (key in inFlight) {
            LocationRules.queueFix(queued, key, fix)
            return
        }
        inFlight.add(key)
        val dir = Native.dataDir(this)
        val app = applicationContext
        LocationShare.work {
            Native.ensure(app)
            val outcome = try {
                JSONObject(Native.reportLocation(dir, share.account, share.shareId, fix) ?: "{}")
            } catch (err: Exception) {
                JSONObject()
            }
            main.post {
                inFlight.remove(key)
                // the share as it is now (its end may have changed), or none once it is no longer sent
                LocationShare.current(key)?.let { took(it, outcome) }
                queued.remove(key)?.let { next -> LocationShare.current(key)?.let { report(it, next) } }
            }
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
            val key = share.key
            main.postDelayed({
                waiting.remove(key)
                LocationShare.current(key)?.let { report(it, "") }
            }, outcome.optLong("waitMs", 5_000L).coerceIn(0L, 3_600_000L))
        }
    }

    /**
     * Each share at its end; every share once the permission is gone; and the notification: with notifications off
     * (Android 13+ lets the person turn them off during a share) every share is stopped on its server, since none is
     * sent without it showing; else it is posted again, so a dismissed one comes back.
     */
    private fun checkShares() {
        val now = System.currentTimeMillis()
        if (!LocationShare.permitted(this)) {
            // the permission taken away while the process lives on (Android usually ends it): stopped on the servers
            Log.i(LocationShare.TAG, "The location permission is gone: the shares stop")
            dropAll()
            return
        }
        var gone = false
        for (share in LocationShare.sent()) {
            val reason = LocationRules.stopReason(share.thruMs, now, true, true, null, null) ?: continue
            Log.i(LocationShare.TAG, "Share ${share.shareId} is no longer sent from here: $reason")
            gone = LocationShare.remove(share.key) || gone
        }
        if (LocationRules.notificationCheck(LocationShare.notificationsShown(this)) == "stop") {
            Log.i(LocationShare.TAG, "Notifications are off: the shares stop")
            LocationShare.stopAll(this)
            end()
            return
        }
        if (gone) changedHere() else post(LocationShare.sent())
    }

    /** Every share no longer sent from here (the permission gone), each stopped on its server, durably. */
    private fun dropAll() {
        LocationShare.stopAll(this)
        end()
    }

    /** The shares changed here: the notification again, or the end with none left. */
    private fun changedHere() {
        val sent = LocationShare.sent()
        if (sent.isEmpty()) {
            end()
            return
        }
        post(sent)
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
        inFlight.clear()
        queued.clear()
        waiting.clear()
        stopForeground(STOP_FOREGROUND_REMOVE)
        inForeground = false
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
