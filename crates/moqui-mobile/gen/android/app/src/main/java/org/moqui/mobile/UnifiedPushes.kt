// This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License.

package org.moqui.mobile

import android.content.Context
import android.util.Log
import org.unifiedpush.android.connector.UnifiedPush

/**
 * UnifiedPush on a phone without Google Play (plans/Comms-Messaging.md §7.4): a distributor app (for example ntfy) gives
 * each account an endpoint of its own (the connector's instance is the account id), which Rust registers with that
 * account's server in place of the FCM token (`Native.unifiedEndpoint`), with the account's own push keys
 * ([RustKeyManager]; [UnifiedPushService] opens the messages with them). A build without Firebase uses the phone's
 * default distributor by itself; the notification check lets the user pick one ([pick]), where Firebase works only
 * after asking, and go back to Firebase ([useFirebase]). [registerAll] registers each account signed in, and takes back
 * the instances of accounts signed out or signing out, at the app's start, at a sign-in or sign-out, and at each run of
 * [RegisterWorker].
 */
object UnifiedPushes {
    private const val TAG = "moqui-push"
    private const val PREFS = "moqui-unifiedpush"

    /** The distributor apps on the phone. */
    fun distributors(context: Context): List<String> = try {
        UnifiedPush.getDistributors(context)
    } catch (err: Throwable) {
        emptyList()
    }

    /** The distributor in use (one that answered), or null. */
    fun distributor(context: Context): String? = try {
        UnifiedPush.getAckDistributor(context)
    } catch (err: Throwable) {
        null
    }

    /** At the app's start: register with the distributor in use, or, with no Firebase, the phone's default one. */
    fun start(context: Context) {
        val app = context.applicationContext
        if (distributor(app) != null) {
            registerAll(app)
            return
        }
        if (BuildConfig.FCM || distributors(app).isEmpty()) return
        try {
            UnifiedPush.tryUseDefaultDistributor(app) { ok ->
                if (ok) registerAll(app) else Log.i(TAG, "No UnifiedPush distributor was taken")
            }
        } catch (err: Throwable) {
            Log.w(TAG, "UnifiedPush did not start", err)
        }
    }

    /**
     * Back to Firebase (the notification check, where the build has it): each account's instance is taken back and the
     * distributor forgotten; the connector's `onUnregistered` hands each account back to the FCM token.
     */
    fun useFirebase(context: Context) {
        val app = context.applicationContext
        Native.work {
            val keys = RustKeyManager(app)
            for (account in Native.accounts(app)) {
                try {
                    UnifiedPush.unregister(app, account, keys)
                } catch (err: Throwable) {
                    Log.w(TAG, "UnifiedPush did not unregister $account", err)
                }
                // at once, should the distributor not answer
                if (Native.ensure(app)) Native.unifiedEndpoint(Native.dataDir(app), account, "")
            }
            try {
                UnifiedPush.removeDistributor(app, keys)
            } catch (err: Throwable) {
                Log.w(TAG, "The UnifiedPush distributor was not forgotten", err)
            }
            app.getSharedPreferences(PREFS, Context.MODE_PRIVATE).edit().remove("instances").apply()
        }
    }

    /**
     * The user picks a distributor (the notification check's fix; where Firebase works, only after the check asked
     * whether to use UnifiedPush in its place); each account registers with it, in place of the FCM token.
     */
    fun pick(context: Context) {
        try {
            UnifiedPush.tryPickDistributor(context) { ok ->
                if (ok) registerAll(context.applicationContext) else Log.i(TAG, "No UnifiedPush distributor was picked")
            }
        } catch (err: Throwable) {
            Log.w(TAG, "No UnifiedPush distributor could be picked", err)
        }
    }

    /**
     * Each account signed in registered with the distributor in use (an instance per account; the connector answers
     * with its endpoint, [UnifiedPushService.onNewEndpoint]), and the instances of accounts no longer here taken back.
     */
    @JvmStatic
    fun registerAll(context: Context) {
        val app = context.applicationContext
        if (distributor(app) == null) return
        Native.work {
            val accounts = Native.accounts(app)
            val keys = RustKeyManager(app)
            val label = app.applicationInfo.loadLabel(app.packageManager).toString()
            val prefs = app.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
            val before = prefs.getStringSet("instances", emptySet()) ?: emptySet()
            for (account in accounts) {
                try {
                    UnifiedPush.register(app, account, label, null, keys)
                } catch (err: Throwable) {
                    Log.w(TAG, "UnifiedPush did not register $account", err)
                }
            }
            for (gone in before - accounts.toSet()) {
                try {
                    UnifiedPush.unregister(app, gone, keys)
                } catch (err: Throwable) {
                    Log.w(TAG, "UnifiedPush did not unregister $gone", err)
                }
            }
            prefs.edit().putStringSet("instances", accounts.toSet()).apply()
        }
    }
}
