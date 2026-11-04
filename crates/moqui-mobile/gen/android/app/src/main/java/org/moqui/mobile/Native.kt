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

    @Volatile private var ready: Boolean? = null

    @JvmStatic external fun init(context: Context): Boolean
    @JvmStatic external fun pushToken(dataDir: String, token: String)
    @JvmStatic external fun decline(dataDir: String, callSessionId: String): String?
    @JvmStatic external fun hasAccount(dataDir: String): Boolean

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
