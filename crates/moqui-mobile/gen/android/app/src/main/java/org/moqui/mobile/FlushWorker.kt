// This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License.

package org.moqui.mobile

import android.content.Context
import android.util.Log
import androidx.work.Constraints
import androidx.work.ExistingWorkPolicy
import androidx.work.NetworkType
import androidx.work.OneTimeWorkRequestBuilder
import androidx.work.WorkManager
import androidx.work.Worker
import androidx.work.WorkerParameters
import androidx.work.workDataOf
import java.util.concurrent.TimeUnit

/**
 * The outbox sent in the background (plans/Mobile-Desktop.md MD6): when the app goes to the background (the activity's
 * stop, and its end, which ends the process), a WorkManager job waits for a network and has Rust take what is due for
 * a bounded time (`Native.flush`, src/native.rs `flush_background`), with the app's process asleep or started for it
 * after it was ended. What is not taken by then stays in the store; when something is still due, or waits for a later
 * time (a retry's backoff), the job runs again, a bounded number of times. In the foreground the app's own sender does
 * this. The go-to-background run and the runs again have their own unique names, so a long wait of one never holds up
 * a new run of the other.
 */
class FlushWorker(context: Context, params: WorkerParameters) : Worker(context, params) {
  override fun doWork(): Result {
    val context = applicationContext
    if (!Native.ensure(context)) return Result.success()
    val next = Native.flush(Native.dataDir(context), BUDGET_MS)
    val round = inputData.getInt(ROUND, 0)
    Log.i(TAG, "Outbox: a background send ran (round $round)")
    if (next >= 0 && round < ROUNDS) {
      Log.i(TAG, "Outbox: more is due in ${next / 1000} s; the background send runs again")
      // a run again after this one: replacing the chain when this is the go-to-background run (any older wait is
      // superseded), else after this run of the chain itself
      val policy = if (round == 0) ExistingWorkPolicy.REPLACE else ExistingWorkPolicy.APPEND_OR_REPLACE
      enqueue(context, AGAIN, maxOf(next, AGAIN_MIN_MS), round + 1, policy)
    }
    return Result.success()
  }

  companion object {
    private const val TAG = "moqui-mobile"
    private const val NAME = "moqui-outbox"
    private const val AGAIN = "moqui-outbox-again"
    private const val ROUND = "round"
    /** How long a run takes operations; one taken goes to its end. WorkManager allows ten minutes. */
    private const val BUDGET_MS = 60_000L
    /** Runs after the first, for what waits for a later time. */
    private const val ROUNDS = 12
    /** The least wait before a run again. */
    private const val AGAIN_MIN_MS = 60_000L

    /**
     * The app went to the background: a new run, replacing one not yet run. With [ending] (the activity finished, and
     * the process ends with it) the run waits a few seconds, so it runs in a process of its own, and this waits up to a
     * second for WorkManager to keep it, before the process ends.
     */
    fun background(context: Context, ending: Boolean = false) {
      val delayMs = if (ending) 5_000L else 0L
      val operation = enqueue(context.applicationContext, NAME, delayMs, 0, ExistingWorkPolicy.REPLACE)
      if (ending && operation != null) {
        try {
          operation.result.get(1, TimeUnit.SECONDS)
        } catch (err: Exception) {
          Log.w(TAG, "The background send was not kept before the app ended", err)
        }
      }
    }

    private fun enqueue(
      context: Context,
      name: String,
      delayMs: Long,
      round: Int,
      policy: ExistingWorkPolicy,
    ): androidx.work.Operation? {
      return try {
        val work = OneTimeWorkRequestBuilder<FlushWorker>()
          .setConstraints(Constraints.Builder().setRequiredNetworkType(NetworkType.CONNECTED).build())
          .setInitialDelay(delayMs, TimeUnit.MILLISECONDS)
          .setInputData(workDataOf(ROUND to round))
          .build()
        WorkManager.getInstance(context).enqueueUniqueWork(name, policy, work)
      } catch (err: Throwable) {
        Log.e(TAG, "The background send was not scheduled", err)
        null
      }
    }
  }
}
