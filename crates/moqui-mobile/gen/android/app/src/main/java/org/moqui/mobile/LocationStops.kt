// This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License.

package org.moqui.mobile

import android.app.NotificationManager
import android.content.Context
import android.util.Log
import androidx.core.app.NotificationCompat
import androidx.work.BackoffPolicy
import androidx.work.Constraints
import androidx.work.ExistingWorkPolicy
import androidx.work.NetworkType
import androidx.work.OneTimeWorkRequestBuilder
import androidx.work.WorkManager
import androidx.work.Worker
import androidx.work.WorkerParameters
import org.json.JSONArray
import org.json.JSONObject
import java.util.concurrent.TimeUnit

/**
 * A live location share stopped from this phone (the notification's Stop, notifications turned off, the permission
 * gone while the process lives on) is stopped on its server too, and that stop is kept until the server confirms it
 * (plans/Location-Sharing.md §3.4): each is kept in the app's preferences with the share's end, tried at once on the
 * location thread, and then by [LocationStopWorker] (WorkManager, with a network, backing off from 30 seconds, also
 * after the process ended), posting `/qapps/stopClientLocation` (`Native.stopLocation`) until the server says it stopped
 * it or has no such share, the account is signed out here, or the share's end has passed ([LocationRules.stopOutcome]).
 * A "Stopping location sharing" notification shows while any waits; one that is let go unconfirmed before its end (the
 * account signed out here with no network) leaves a notice saying until when the share shows the last position. The
 * server also stops a revoked device key's shares itself (`stop#DeviceShares`).
 */
object LocationStops {
    private const val PREFS = "moqui-location"
    private const val PENDING = "stopsWaiting"
    internal const val NOTIFICATION_ID = 7304
    internal const val NOTICE_ID = 7305
    private const val WORK = "moqui-location-stop"

    /** The stops waiting, each share's key with its end (epoch millis). */
    @Synchronized
    fun pending(context: Context): Map<String, Long> {
        val text = context.getSharedPreferences(PREFS, Context.MODE_PRIVATE).getString(PENDING, "{}") ?: "{}"
        return try {
            val json = JSONObject(text)
            json.keys().asSequence().associateWith { json.optLong(it) }
        } catch (err: Exception) {
            emptyMap()
        }
    }

    @Synchronized
    private fun keep(context: Context, stops: Map<String, Long>) {
        val json = JSONObject()
        stops.forEach { (key, thru) -> json.put(key, thru) }
        context.getSharedPreferences(PREFS, Context.MODE_PRIVATE).edit().putString(PENDING, json.toString()).apply()
    }

    /** The share `shareId` of `account`, until `thruMs`, is to be stopped on its server: kept, shown, tried at once and scheduled. */
    fun add(context: Context, account: String, shareId: String, thruMs: Long) {
        val app = context.applicationContext
        val key = LocationRules.stopKey(account, shareId)
        synchronized(this) { keep(app, pending(app) + (key to thruMs)) }
        show(app)
        // once now on the location thread, then by WorkManager for as long as it takes
        LocationShare.work { tryOne(app, key) }
        schedule(app)
    }

    /** One try of the stop of `key`: what it came to. Not on the main thread. */
    internal fun tryOne(context: Context, key: String): String {
        val (account, shareId) = LocationRules.parseStopKey(key) ?: return settle(context, key, "gone")
        if (!pending(context).containsKey(key)) return "gone"
        if (!Native.ensure(context)) return "retry"
        val state = try {
            Native.stopLocation(Native.dataDir(context), account, shareId) ?: "retry"
        } catch (err: Throwable) {
            "retry"
        }
        Log.i(LocationShare.TAG, "The stop of share $shareId on its server: $state")
        return settle(context, key, state)
    }

    /**
     * What a stop of `key` came to: kept while it is to be tried again, else let go, with a notice when it was let go
     * unconfirmed before the share's end. The state.
     */
    fun settle(context: Context, key: String, state: String): String {
        val thru = synchronized(this) {
            val waiting = pending(context)
            val thru = waiting[key] ?: return state
            val outcome = LocationRules.stopOutcome(state, thru, System.currentTimeMillis())
            if (outcome != "keep") keep(context, waiting - key)
            if (outcome == "unconfirmed") thru else null
        }
        if (thru != null) notice(context, thru)
        show(context)
        return state
    }

    /** The shares of `account` whose stop waits, a JSON list of ids (posted at its sign-out, `account_signing_out`). */
    @JvmStatic
    fun pendingFor(context: Context, account: String): String = JSONArray(
        pending(context).keys.mapNotNull { LocationRules.parseStopKey(it) }.filter { it.first == account }.map { it.second }
    ).toString()

    /** What a stop of `account`'s share came to at its sign-out. */
    @JvmStatic
    fun settledFor(context: Context, account: String, shareId: String, state: String) {
        settle(context.applicationContext, LocationRules.stopKey(account, shareId), state)
    }

    fun schedule(context: Context) {
        try {
            val work = OneTimeWorkRequestBuilder<LocationStopWorker>()
                .setConstraints(Constraints.Builder().setRequiredNetworkType(NetworkType.CONNECTED).build())
                .setBackoffCriteria(BackoffPolicy.EXPONENTIAL, 30, TimeUnit.SECONDS)
                .build()
            WorkManager.getInstance(context).enqueueUniqueWork(WORK, ExistingWorkPolicy.REPLACE, work)
        } catch (err: Throwable) {
            Log.e(LocationShare.TAG, "The location stop was not scheduled", err)
        }
    }

    /** "Stopping location sharing…" while a stop waits for its server; gone once none waits. */
    fun show(context: Context) {
        val manager = context.getSystemService(NotificationManager::class.java) ?: return
        if (pending(context).isEmpty()) {
            manager.cancel(NOTIFICATION_ID)
            return
        }
        LocationShare.ensureChannel(context)
        val notification = NotificationCompat.Builder(context, LocationShare.CHANNEL)
            .setSmallIcon(android.R.drawable.ic_menu_mylocation)
            .setContentTitle(context.getString(R.string.location_stopping_title))
            .setContentText(context.getString(R.string.location_stopping_text))
            .setOngoing(true)
            .setOnlyAlertOnce(true)
            .build()
        try {
            manager.notify(NOTIFICATION_ID, notification)
        } catch (err: SecurityException) {
            // notifications not allowed: the stop is still tried
            Log.i(LocationShare.TAG, "The stopping notification is not shown: ${err.message}")
        }
    }

    /** A stop let go before the server heard of it: until when the share shows the last position. */
    private fun notice(context: Context, thruMs: Long) {
        val manager = context.getSystemService(NotificationManager::class.java) ?: return
        LocationShare.ensureChannel(context)
        val until = LocationRules.untilFallback(thruMs, System.currentTimeMillis())
        val text = context.getString(R.string.location_stop_unconfirmed, until)
        val notification = NotificationCompat.Builder(context, LocationShare.CHANNEL)
            .setSmallIcon(android.R.drawable.ic_menu_mylocation)
            .setContentTitle(context.getString(R.string.location_stop_unconfirmed_title))
            .setContentText(text)
            .setStyle(NotificationCompat.BigTextStyle().bigText(text))
            .setAutoCancel(true)
            .build()
        try {
            manager.notify(NOTICE_ID, notification)
        } catch (err: SecurityException) {
            Log.i(LocationShare.TAG, "The unconfirmed stop's notice is not shown: ${err.message}")
        }
    }
}

/** The stops of [LocationStops], each posted until it settles; tried again with the network. */
class LocationStopWorker(context: Context, params: WorkerParameters) : Worker(context, params) {
    override fun doWork(): Result {
        val app = applicationContext
        if (!Native.ensure(app)) return Result.retry()
        for (key in LocationStops.pending(app).keys) LocationStops.tryOne(app, key)
        LocationStops.show(app)
        return if (LocationStops.pending(app).isEmpty()) Result.success() else Result.retry()
    }
}
