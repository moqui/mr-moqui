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
import java.util.concurrent.TimeUnit

/**
 * A live location share stopped from this phone (the notification's Stop, or notifications turned off) is stopped on
 * its server too, and that stop is kept until the server confirms it (plans/Location-Sharing.md §3.4): each is kept in
 * the app's preferences, and [LocationStopWorker] (WorkManager, with a network, backing off from 30 seconds, also after
 * the process ended) posts `/qapps/stopClientLocation` (`Native.stopLocation`) until the server says it stopped it, has
 * no such share, or the account is signed out here ([LocationRules.stopSettled]). A "Stopping location sharing"
 * notification shows while any waits, so the person knows the share is not off yet.
 */
object LocationStops {
    private const val PREFS = "moqui-location"
    private const val PENDING = "stopsPending"
    internal const val NOTIFICATION_ID = 7304
    private const val WORK = "moqui-location-stop"

    @Synchronized
    fun pending(context: Context): Set<String> =
        context.getSharedPreferences(PREFS, Context.MODE_PRIVATE).getStringSet(PENDING, emptySet())?.toSet() ?: emptySet()

    @Synchronized
    private fun keep(context: Context, keys: Set<String>) {
        context.getSharedPreferences(PREFS, Context.MODE_PRIVATE).edit().putStringSet(PENDING, keys).apply()
    }

    /** The share `shareId` of `account` is to be stopped on its server: kept, shown, and the worker scheduled. */
    fun add(context: Context, account: String, shareId: String) {
        val app = context.applicationContext
        synchronized(this) { keep(app, pending(app) + LocationRules.stopKey(account, shareId)) }
        show(app)
        schedule(app)
    }

    /** What a stop came to (`stopped`, `gone`, `signedOut`, `retry`): kept until it settles. What is left. */
    fun answered(context: Context, key: String, state: String): Set<String> = synchronized(this) {
        val left = LocationRules.stopsAfter(pending(context), key, state)
        keep(context, left)
        left
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
}

/** The stops of [LocationStops], each posted until its server settles it; tried again with the network. */
class LocationStopWorker(context: Context, params: WorkerParameters) : Worker(context, params) {
    override fun doWork(): Result {
        val app = applicationContext
        if (!Native.ensure(app)) return Result.retry()
        val dir = Native.dataDir(app)
        var left = LocationStops.pending(app)
        for (key in left) {
            val parsed = LocationRules.parseStopKey(key)
            if (parsed == null) {
                left = LocationStops.answered(app, key, "gone")
                continue
            }
            val (account, shareId) = parsed
            val state = try {
                Native.stopLocation(dir, account, shareId) ?: "retry"
            } catch (err: Throwable) {
                "retry"
            }
            Log.i(LocationShare.TAG, "The stop of share $shareId on its server: $state")
            left = LocationStops.answered(app, key, state)
        }
        LocationStops.show(app)
        return if (left.isEmpty()) Result.success() else Result.retry()
    }
}
