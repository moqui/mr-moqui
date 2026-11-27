// This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License.

package org.moqui.mobile

import android.content.Context
import android.util.Log
import androidx.work.BackoffPolicy
import androidx.work.Constraints
import androidx.work.ExistingWorkPolicy
import androidx.work.NetworkType
import androidx.work.OneTimeWorkRequest
import androidx.work.OneTimeWorkRequestBuilder
import androidx.work.WorkManager
import androidx.work.Worker
import androidx.work.WorkerParameters
import androidx.work.workDataOf
import java.util.concurrent.TimeUnit

/**
 * An offline map package downloaded in the background (plans/Maps-Offline.md §2.4, OF2): the shell's
 * `packages_download` hands it here (src/host.rs, `PackageDownloadWorker.start` through JNI), and the worker has Rust
 * download it (`Native.packageDownload`, src/native.rs `package_download`): ranges into a part file resumed from its
 * length, the checks, the unpack and the swap, with the account's key, as the outbox's [FlushWorker] sends with it.
 * Resumable: a cut, a server error or WorkManager's stop (its network lost, Wi-Fi gone with "only on Wi-Fi", its ten
 * minutes up) pauses the run with its part kept (`Native.packagePause`), and the next run goes on from there, backing
 * off from 30 seconds. One work per account and package; a new revision asked replaces the run of another.
 */
class PackageDownloadWorker(context: Context, params: WorkerParameters) : Worker(context, params) {
  override fun doWork(): Result {
    val app = applicationContext
    val account = inputData.getString(ACCOUNT) ?: return Result.failure()
    val packageId = inputData.getString(PACKAGE) ?: return Result.failure()
    val revision = inputData.getLong(REVISION, -1L)
    if (revision < 0) return Result.failure()
    if (!Native.ensure(app)) return Result.retry()
    val ended = try {
      Native.packageDownload(Native.dataDir(app), account, packageId, revision)
    } catch (err: Throwable) {
      Log.e(TAG, "The offline map $packageId was not downloaded", err)
      PackageDownloadRules.RETRY
    }
    return when (PackageDownloadRules.outcome(ended, runAttemptCount, isStopped)) {
      PackageDownloadRules.Outcome.SUCCESS -> Result.success()
      PackageDownloadRules.Outcome.RETRY -> {
        Log.i(TAG, "The offline map $packageId goes on later (attempt ${runAttemptCount + 1})")
        Result.retry()
      }
      PackageDownloadRules.Outcome.FAILURE -> Result.failure()
    }
  }

  override fun onStopped() {
    // WorkManager stopped the run (its constraints, its time): Rust stops at its next range and keeps the part
    val account = inputData.getString(ACCOUNT) ?: return
    val packageId = inputData.getString(PACKAGE) ?: return
    try {
      Native.packagePause(account, packageId)
    } catch (err: Throwable) {
      Log.w(TAG, "The offline map $packageId was not paused", err)
    }
  }

  companion object {
    private const val TAG = "moqui-mobile"
    const val ACCOUNT = "account"
    const val PACKAGE = "packageId"
    const val REVISION = "revision"

    /** The work of a revision of an account's package: with a network, Wi-Fi alone when [wifiOnly], storage not low. */
    fun request(account: String, packageId: String, revision: Long, wifiOnly: Boolean): OneTimeWorkRequest =
      OneTimeWorkRequestBuilder<PackageDownloadWorker>()
        .setConstraints(PackageDownloadRules.constraints(wifiOnly))
        .setBackoffCriteria(BackoffPolicy.EXPONENTIAL, PackageDownloadRules.BACKOFF_SECONDS, TimeUnit.SECONDS)
        .setInputData(workDataOf(ACCOUNT to account, PACKAGE to packageId, REVISION to revision))
        .addTag(PackageDownloadRules.accountTag(account))
        .build()

    /** Download `revision` of a package (the shell's `packages_download`): replaces a run of the package not done. */
    @JvmStatic
    fun start(context: Context, account: String, packageId: String, revision: Long, wifiOnly: Boolean): Boolean =
      try {
        WorkManager.getInstance(context.applicationContext).enqueueUniqueWork(
          PackageDownloadRules.uniqueName(account, packageId),
          ExistingWorkPolicy.REPLACE,
          request(account, packageId, revision, wifiOnly),
        )
        true
      } catch (err: Throwable) {
        Log.e(TAG, "The offline map's download was not scheduled", err)
        false
      }

    /** A package's download cancelled or the package deleted. */
    @JvmStatic
    fun cancel(context: Context, account: String, packageId: String) {
      try {
        WorkManager.getInstance(context.applicationContext)
          .cancelUniqueWork(PackageDownloadRules.uniqueName(account, packageId))
      } catch (err: Throwable) {
        Log.w(TAG, "The offline map's download was not cancelled", err)
      }
    }

    /** Every download of an account (its data deleted: a shared device's sign-out, a removal). */
    @JvmStatic
    fun cancelAccount(context: Context, account: String) {
      try {
        WorkManager.getInstance(context.applicationContext).cancelAllWorkByTag(PackageDownloadRules.accountTag(account))
      } catch (err: Throwable) {
        Log.w(TAG, "The account's offline map downloads were not cancelled", err)
      }
    }
  }
}

/** The worker's rules, with no Android in them but WorkManager's own classes (JVM-tested: PackageDownloadRulesTest). */
object PackageDownloadRules {
  /** What `Native.packageDownload` answers: done (installed, or ended for good: refused, cancelled, stale)... */
  const val DONE = 0
  /** ...or to be tried again (a cut, a server error, a pause): the part file is kept, and the next run resumes it. */
  const val RETRY = 1
  /** Runs at most before a download that keeps failing is given up (its part stays for a download asked again). */
  const val MAX_ATTEMPTS = 20
  const val BACKOFF_SECONDS = 30L

  enum class Outcome { SUCCESS, RETRY, FAILURE }

  /** A network for every download; an unmetered one (Wi-Fi) for one asked "only on Wi-Fi"; never on low storage. */
  fun networkType(wifiOnly: Boolean): NetworkType = if (wifiOnly) NetworkType.UNMETERED else NetworkType.CONNECTED

  fun constraints(wifiOnly: Boolean): Constraints =
    Constraints.Builder().setRequiredNetworkType(networkType(wifiOnly)).setRequiresStorageNotLow(true).build()

  /** What WorkManager is told of a run that answered [result] on its [attempt] (0 the first), stopped or not. */
  fun outcome(result: Int, attempt: Int, stopped: Boolean): Outcome =
    when {
      // stopped by WorkManager: it runs the work again itself, from the kept part
      stopped -> Outcome.RETRY
      result == RETRY && attempt + 1 < MAX_ATTEMPTS -> Outcome.RETRY
      result == RETRY -> Outcome.FAILURE
      else -> Outcome.SUCCESS
    }

  /** One work per account and package. */
  fun uniqueName(account: String, packageId: String): String = "moqui-map-package/$account/$packageId"

  /** The tag of an account's downloads. */
  fun accountTag(account: String): String = "moqui-map-packages/$account"
}
