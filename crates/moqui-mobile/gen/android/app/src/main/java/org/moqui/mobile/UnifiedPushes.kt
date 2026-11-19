// This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License.

package org.moqui.mobile

import android.content.Context
import android.util.Log
import org.unifiedpush.android.connector.UnifiedPush

/**
 * UnifiedPush on a phone without Google Play (plans/Comms-Messaging.md §7.4): a distributor app (for example ntfy) gives
 * each account an endpoint of its own, which Rust registers with that account's server in place of the FCM token
 * (`Native.unifiedEndpoint`), with the account's own push keys ([RustKeyManager]; [UnifiedPushService] opens the
 * messages with them). The connector's instance, the name the distributor knows an account by, is random and kept in
 * Rust's `push.json` (`Native.instanceOf`), not the account id, which is the server's host and a hash of the server and
 * username (SEC-CLI-125). A build without Firebase uses the phone's default distributor by itself; the notification
 * check lets the user pick one ([pick]), where Firebase works only after asking, and go back to Firebase
 * ([useFirebase]). [registerAll] registers each account signed in, and takes back the instances of accounts signed out
 * or signing out, at the app's start, at a sign-in or sign-out, and at each run of [RegisterWorker].
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
     * Back to Firebase (the notification check, where the build has it): each instance registered ([registerAll]) is
     * taken back and the distributor forgotten; each account goes back to the FCM token at once (the connector's
     * `onUnregistered` would do it too).
     */
    fun useFirebase(context: Context) {
        val app = context.applicationContext
        Native.work {
            val keys = RustKeyManager(app)
            val prefs = app.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
            val instances = prefs.getStringSet("instances", emptySet()) ?: emptySet()
            for (instance in instances) {
                try {
                    UnifiedPush.unregister(app, instance, keys)
                } catch (err: Throwable) {
                    Log.w(TAG, "UnifiedPush did not unregister $instance", err)
                }
            }
            // at once, should the distributor not answer
            if (Native.ensure(app)) {
                val dir = Native.dataDir(app)
                for (account in Native.accounts(app)) Native.unifiedEndpoint(dir, account, "")
            }
            try {
                UnifiedPush.removeDistributor(app, keys)
            } catch (err: Throwable) {
                Log.w(TAG, "The UnifiedPush distributor was not forgotten", err)
            }
            prefs.edit().remove("instances").apply()
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
     * Each account signed in registered with the distributor in use under its own instance (`Native.instanceOf`; the
     * connector answers with its endpoint, [UnifiedPushService.onNewEndpoint]), and the instances no longer any
     * account's taken back: those of accounts signed out, and an earlier build's, which were the account ids (their
     * accounts' endpoints stay until the new instances' come, and the server's registration of the new one replaces the
     * old one, as it keeps one a device key; a push sent to an old instance meanwhile is lost).
     */
    @JvmStatic
    fun registerAll(context: Context) {
        val app = context.applicationContext
        if (distributor(app) == null) return
        Native.work {
            val accounts = Native.accounts(app)
            val dir = Native.dataDir(app)
            val keys = RustKeyManager(app)
            val label = app.applicationInfo.loadLabel(app.packageManager).toString()
            val prefs = app.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
            val before = prefs.getStringSet("instances", emptySet()) ?: emptySet()
            val instances = mutableSetOf<String>()
            for (account in accounts) {
                val instance = Native.instanceOf(dir, account)
                if (instance == null) {
                    Log.w(TAG, "UnifiedPush has no instance for $account")
                    continue
                }
                instances.add(instance)
                try {
                    UnifiedPush.register(app, instance, label, null, keys)
                } catch (err: Throwable) {
                    Log.w(TAG, "UnifiedPush did not register $account", err)
                }
            }
            for (gone in before - instances) {
                try {
                    UnifiedPush.unregister(app, gone, keys)
                } catch (err: Throwable) {
                    Log.w(TAG, "UnifiedPush did not unregister $gone", err)
                }
            }
            prefs.edit().putStringSet("instances", instances).apply()
        }
    }
}
