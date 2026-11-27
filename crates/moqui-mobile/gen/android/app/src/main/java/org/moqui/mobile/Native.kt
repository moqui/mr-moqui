// This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License.

package org.moqui.mobile

import android.content.Context
import android.util.Log
import java.util.concurrent.Executors

/**
 * The app's Rust library for the native code (crates/moqui-mobile/src/native.rs), with or without the activity: a push
 * can start the process with none. [ensure] loads it and hands it the application context once; each call that makes
 * a request runs on [work]'s thread, never the main one.
 */
object Native {
    private const val TAG = "moqui-calls"
    private val worker = Executors.newSingleThreadExecutor { task -> Thread(task, "moqui-native") }
    /** Opening a push (no network): a thread of its own, so a ring never waits behind a registration's posts. */
    private val opener = Executors.newSingleThreadExecutor { task -> Thread(task, "moqui-push-open") }

    @Volatile private var ready: Boolean? = null

    @JvmStatic external fun init(context: Context): Boolean
    @JvmStatic external fun pushToken(dataDir: String, token: String)
    @JvmStatic external fun decline(dataDir: String, callSessionId: String, account: String): String?
    @JvmStatic external fun ringAccount(dataDir: String, pushId: String): String?
    @JvmStatic external fun callLink(callSessionId: String, account: String, video: Boolean, hangUp: Boolean): String?
    @JvmStatic external fun hangUp(callSessionId: String)
    /** A number the phone's dialer handed the app ([DialAccount]): the nonce of its link, or null for no number. */
    @JvmStatic external fun dialLink(number: String): String?
    @JvmStatic external fun hasAccount(dataDir: String): Boolean
    @JvmStatic external fun flush(dataDir: String, budgetMs: Long): Long
    @JvmStatic external fun unifiedEndpoint(dataDir: String, account: String, endpoint: String)
    @JvmStatic external fun registerPending(dataDir: String): Boolean
    @JvmStatic external fun openPush(dataDir: String, pushId: String, kind: String, enc: String): String?
    @JvmStatic external fun openFor(dataDir: String, account: String, body: ByteArray): ByteArray?
    @JvmStatic external fun pushPublic(dataDir: String, account: String, make: Boolean): String?
    /**
     * The UnifiedPush instance of `account` ([UnifiedPushes]: random, kept in `push.json`, made the first time), or
     * null for an account not signed in here. The distributor app knows the account only by it, never by its id.
     */
    @JvmStatic external fun instanceOf(dataDir: String, account: String): String?
    /** The account of a UnifiedPush `instance` made by [instanceOf], or null (an account id is no instance). */
    @JvmStatic external fun accountOf(dataDir: String, instance: String): String?
    @JvmStatic external fun accountIds(dataDir: String): String?
    @JvmStatic external fun pushKnown(dataDir: String, pushId: String): Boolean
    @JvmStatic external fun roomLink(dataDir: String, account: String, chatRoomId: String, eventSeq: Long): String?
    @JvmStatic external fun chatReply(dataDir: String, account: String, chatRoomId: String, text: String): String?
    @JvmStatic external fun chatRead(dataDir: String, account: String, chatRoomId: String, eventSeq: Long): String?
    @JvmStatic external fun ackPush(dataDir: String, account: String, ackId: String)
    @JvmStatic external fun vaultLock()
    /**
     * Download `revision` of an account's offline map package ([PackageDownloadWorker], plans/Maps-Offline.md §2.4), on
     * the worker's thread: [PackageDownloadRules.DONE] when it ended (installed, refused, cancelled, stale), or
     * [PackageDownloadRules.RETRY] to be tried again from its kept part.
     */
    @JvmStatic external fun packageDownload(dataDir: String, account: String, packageId: String, revision: Long): Int
    /** WorkManager stopped a package's download: Rust stops at its next range and keeps its part. */
    @JvmStatic external fun packagePause(account: String, packageId: String)

    /**
     * The app's screen went (the phone locked, or another app came to the front): every vault session locks
     * (plans/archive/Vault-Instances.md V5). Nothing to lock before the library is loaded.
     */
    fun lockVault() {
        if (ready != true) return
        try {
            vaultLock()
        } catch (err: Throwable) {
            Log.e(TAG, "The vault was not locked", err)
        }
    }

    /** The library loaded and given the context; whether it may make requests (its TLS verifier started). */
    @Synchronized
    fun ensure(context: Context): Boolean {
        ready?.let { return it }
        val done = try {
            System.loadLibrary("moqui_mobile")
            init(context.applicationContext)
        } catch (err: Throwable) {
            Log.e(TAG, "The app's library did not load", err)
            false
        }
        ready = done
        return done
    }

    /** The app's data directory, whose `client` is the Rust client's (Tauri's app_data_dir). */
    fun dataDir(context: Context): String = context.applicationContext.dataDir.absolutePath

    /** `task` on the push opener's thread (never network work); a failure is logged. */
    fun open(task: () -> Unit) {
        opener.execute {
            try {
                task()
            } catch (err: Throwable) {
                Log.e(TAG, "Opening a push failed", err)
            }
        }
    }

    @JvmStatic external fun callTell(what: String, json: String)

    /** An encoded VP8 frame of the screen share for its leg (`ScreenShare`, Rust's `screen.rs`). */
    @JvmStatic external fun screenFrame(data: ByteArray, ptsUs: Long)

    /** The screen share's projection stopped: its leg goes. */
    @JvmStatic external fun screenEnded()

    /**
     * Tell the shell's call screen what the native call did (`audio`: its route and routes; `pip`: picture-in-picture
     * on or off; `hold`: Telecom holds or resumes the call), when the library is loaded; nothing otherwise.
     */
    fun tellPage(what: String, json: String) {
        if (ready != true) return
        try {
            callTell(what, json)
        } catch (err: Throwable) {
            Log.w(TAG, "The page was not told of $what", err)
        }
    }

    /** The accounts signed in here. */
    fun accounts(context: Context): List<String> {
        if (!ensure(context)) return emptyList()
        return accountIds(dataDir(context))?.split('\n')?.filter { it.isNotBlank() } ?: emptyList()
    }

    /** `task` on the native code's own thread; a failure is logged. */
    fun work(task: () -> Unit) {
        worker.execute {
            try {
                task()
            } catch (err: Throwable) {
                Log.e(TAG, "Native work failed", err)
            }
        }
    }
}
