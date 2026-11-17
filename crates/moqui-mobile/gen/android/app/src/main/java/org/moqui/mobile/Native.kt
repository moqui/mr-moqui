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
    @JvmStatic external fun hasAccount(dataDir: String): Boolean
    @JvmStatic external fun flush(dataDir: String, budgetMs: Long): Long
    @JvmStatic external fun unifiedEndpoint(dataDir: String, account: String, endpoint: String)
    @JvmStatic external fun registerPending(dataDir: String): Boolean
    @JvmStatic external fun openPush(dataDir: String, pushId: String, kind: String, enc: String): String?
    @JvmStatic external fun openFor(dataDir: String, account: String, body: ByteArray): ByteArray?
    @JvmStatic external fun pushPublic(dataDir: String, account: String, make: Boolean): String?
    @JvmStatic external fun accountIds(dataDir: String): String?
    @JvmStatic external fun pushKnown(dataDir: String, pushId: String): Boolean
    @JvmStatic external fun roomLink(dataDir: String, account: String, chatRoomId: String, eventSeq: Long): String?
    @JvmStatic external fun chatReply(dataDir: String, account: String, chatRoomId: String, text: String): String?
    @JvmStatic external fun chatRead(dataDir: String, account: String, chatRoomId: String, eventSeq: Long): String?
    @JvmStatic external fun ackPush(dataDir: String, account: String, ackId: String)

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
