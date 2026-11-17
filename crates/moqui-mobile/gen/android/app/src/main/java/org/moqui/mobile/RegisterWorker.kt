// This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License.

package org.moqui.mobile

import android.content.Context
import android.util.Log
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
 * The push token's registration, tried again until each account's server answers (plans/Comms-Messaging.md §7.4,
 * Token): at every start and every new token ([Push]), with the network, backing off from 30 seconds
 * (`Native.registerPending`, src/native.rs register_pending).
 */
class RegisterWorker(context: Context, params: WorkerParameters) : Worker(context, params) {
    override fun doWork(): Result {
        val app = applicationContext
        if (!Native.ensure(app)) return Result.retry()
        return if (Native.registerPending(Native.dataDir(app))) {
            Result.success()
        } else {
            Log.i(ChatNotifications.TAG, "The push token is not registered with every account yet: trying again")
            Result.retry()
        }
    }

    companion object {
        private const val NAME = "moqui-push-register"

        fun start(context: Context) {
            try {
                val work = OneTimeWorkRequestBuilder<RegisterWorker>()
                    .setConstraints(Constraints.Builder().setRequiredNetworkType(NetworkType.CONNECTED).build())
                    .setBackoffCriteria(BackoffPolicy.EXPONENTIAL, 30, TimeUnit.SECONDS)
                    .build()
                WorkManager.getInstance(context.applicationContext).enqueueUniqueWork(NAME, ExistingWorkPolicy.REPLACE, work)
            } catch (err: Throwable) {
                Log.e(ChatNotifications.TAG, "The push registration was not scheduled", err)
            }
        }
    }
}
