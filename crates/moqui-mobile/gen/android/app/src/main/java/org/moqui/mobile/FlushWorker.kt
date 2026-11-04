// This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License.

package org.moqui.mobile

import android.content.Context
import android.util.Log
import androidx.lifecycle.DefaultLifecycleObserver
import androidx.lifecycle.LifecycleOwner
import androidx.lifecycle.ProcessLifecycleOwner
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
 * The outbox sent in the background (plans/Mobile-Desktop.md MD6): when the app goes to the background, a WorkManager
 * job waits for a network and has Rust send what is due for a bounded time (`Native.flush`, src/native.rs
 * `flush_background`), with the app's process asleep or started for it after it was ended. What is not sent by then
 * stays in the store; when something waits for a later time (a retry's backoff), the job runs again after it, a
 * bounded number of times. In the foreground the app's own sender does this.
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
      enqueue(context, maxOf(next, AGAIN_MIN_MS), round + 1, ExistingWorkPolicy.APPEND_OR_REPLACE)
    }
    return Result.success()
  }

  companion object {
    private const val TAG = "moqui-mobile"
    private const val NAME = "moqui-outbox"
    private const val ROUND = "round"
    /** How long a run takes operations; one taken goes to its end. WorkManager allows ten minutes. */
    private const val BUDGET_MS = 60_000L
    /** Runs after the first, for what waits for a later time. */
    private const val ROUNDS = 12
    /** The least wait before a run again. */
    private const val AGAIN_MIN_MS = 60_000L

    @Volatile private var watching = false

    /** Schedule a background send each time the app goes to the background. On the main thread (the activity's start). */
    fun watch(context: Context) {
      if (watching) return
      watching = true
      val app = context.applicationContext
      ProcessLifecycleOwner.get().lifecycle.addObserver(object : DefaultLifecycleObserver {
        override fun onStop(owner: LifecycleOwner) {
          enqueue(app, 0, 0, ExistingWorkPolicy.KEEP)
        }
      })
    }

    private fun enqueue(context: Context, delayMs: Long, round: Int, policy: ExistingWorkPolicy) {
      try {
        val work = OneTimeWorkRequestBuilder<FlushWorker>()
          .setConstraints(Constraints.Builder().setRequiredNetworkType(NetworkType.CONNECTED).build())
          .setInitialDelay(delayMs, TimeUnit.MILLISECONDS)
          .setInputData(workDataOf(ROUND to round))
          .build()
        WorkManager.getInstance(context).enqueueUniqueWork(NAME, policy, work)
      } catch (err: Throwable) {
        Log.e(TAG, "The background send was not scheduled", err)
      }
    }
  }
}
