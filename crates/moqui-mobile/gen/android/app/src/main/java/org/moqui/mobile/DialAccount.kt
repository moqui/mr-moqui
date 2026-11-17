// This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License.

package org.moqui.mobile

import android.app.NotificationManager
import android.app.PendingIntent
import android.content.ComponentName
import android.content.Context
import android.content.Intent
import android.graphics.drawable.Icon
import android.net.Uri
import android.os.Handler
import android.os.Looper
import android.telecom.Connection
import android.telecom.ConnectionRequest
import android.telecom.DisconnectCause
import android.telecom.PhoneAccount
import android.telecom.PhoneAccountHandle
import android.telecom.TelecomManager
import android.util.Log
import androidx.core.app.NotificationCompat

/**
 * The app as a calling account of the phone (plans/Comms-Messaging.md §9.1): a managed `tel:` PhoneAccount
 * (CAPABILITY_CALL_PROVIDER, android.telecom: androidx.core.telecom has no managed account), so the phone's dialer and
 * Contacts can call a number through the app. It is registered while an account signed in here has a number to call from
 * (the shell says so, `call_dial_account` in src/commands.rs), and Android lists it under Calling accounts, where the
 * user turns it on (the app cannot, nor read whether it is without READ_PHONE_NUMBERS, which it does not ask: a number
 * that came through it says so): the notification check says so and opens that screen ([fix]).
 *
 * A number dialed with it is handed to the app, never called by Telecom: Telecom's call is cancelled at once, and the app
 * opens at its Dialer with the number, which places it once through the server (`place#PhoneCall`, with its caller ID,
 * fraud limits and emergency path). The link carries only a nonce Rust made for the number ([Native.dialLink]), so
 * another app cannot have the app call a number of its choice. Emergency numbers never come here: Telecom places them
 * on the SIM.
 */
object DialAccount {
    const val ACCOUNT_ID = "moqui-dial"
    private const val TAG = "moqui-calls"
    /** The notification that hands a number over when Android did not let the app open by itself. */
    private const val HANDOFF_ID = 4_101
    private const val HANDOFF_WAIT_MS = 3_000L
    private const val PREFS = "moqui-dial"

    /** How the app is opened with a number's link (a test stands in for it). */
    @Volatile var opener: (Context, Intent) -> Unit = { context, intent -> context.startActivity(intent) }

    fun handle(context: Context): PhoneAccountHandle =
        PhoneAccountHandle(ComponentName(context, CallConnectionService::class.java), ACCOUNT_ID)

    fun isDial(handle: PhoneAccountHandle?): Boolean = handle?.id == ACCOUNT_ID

    /**
     * The calling account registered (`on`: an account here has a number to call from) or taken away (none has); Rust
     * calls it through JNI. Registering again keeps the user's choice of turning it on. Whether Telecom took it.
     */
    @JvmStatic
    fun set(context: Context, on: Boolean): Boolean {
        val telecom = context.getSystemService(TelecomManager::class.java) ?: return false
        val handle = handle(context)
        return try {
            if (on) {
                telecom.registerPhoneAccount(
                    PhoneAccount.builder(handle, context.getString(R.string.dial_account_label))
                        .setCapabilities(PhoneAccount.CAPABILITY_CALL_PROVIDER)
                        .addSupportedUriScheme(PhoneAccount.SCHEME_TEL)
                        .setShortDescription(context.getString(R.string.dial_account_about))
                        .setIcon(Icon.createWithResource(context, R.mipmap.ic_launcher))
                        .build()
                )
            } else {
                telecom.unregisterPhoneAccount(handle)
            }
            // whether it is turned on is not the app's to read (Telecom asks READ_PHONE_NUMBERS of that): a number that came
            // through it says it is; a new registration is off until then
            val kept = context.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
            val prefs = kept.edit().putBoolean("registered", on)
            if (!on || !kept.getBoolean("registered", false)) prefs.putBoolean("used", false)
            prefs.apply()
            Log.i(TAG, if (on) "The calling account is registered" else "The calling account is taken away")
            true
        } catch (err: Exception) {
            Log.w(TAG, "The calling account was not ${if (on) "registered" else "taken away"}: ${err.message}")
            false
        }
    }

    /**
     * The check's state of the account: `off` (not registered), `registered` (the user may not have turned it on yet) or
     * `on` (a number came through it).
     */
    fun state(context: Context): String {
        val prefs = context.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
        return CallRules.dialAccountState(prefs.getBoolean("registered", false), prefs.getBoolean("used", false))
    }

    /** The check's fix: Android's Calling accounts screen, where the user turns the app's account on. */
    fun fix(context: Context) {
        val intents = listOf(
            Intent(TelecomManager.ACTION_CHANGE_PHONE_ACCOUNTS),
            Intent(android.provider.Settings.ACTION_SETTINGS),
        )
        for (intent in intents) {
            try {
                context.startActivity(intent.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
                return
            } catch (err: Exception) {
                Log.w(TAG, "The calling accounts screen did not open: ${err.message}")
            }
        }
    }

    /**
     * The phone's dialer or Contacts called a number with the app's account ([CallConnectionService]): the number goes
     * to the app's Dialer, and Telecom's own call is cancelled.
     */
    fun dialed(context: Context, request: ConnectionRequest?): Connection {
        val app = context.applicationContext
        CallManager.know(app)
        // the user turned the account on: a number came through it
        app.getSharedPreferences(PREFS, Context.MODE_PRIVATE).edit().putBoolean("used", true).apply()
        val number = request?.address?.schemeSpecificPart ?: ""
        val nonce = if (Native.ensure(app)) Native.dialLink(number) else null
        if (nonce.isNullOrEmpty()) {
            Log.w(TAG, "A number from the phone's dialer that the app does not call")
            return Connection.createFailedConnection(
                DisconnectCause(DisconnectCause.ERROR, app.getString(R.string.dial_not_called), app.getString(R.string.dial_not_called), "not a number")
            )
        }
        val open = Intent(app, MainActivity::class.java)
            .setAction(Intent.ACTION_VIEW)
            .setData(Uri.parse("org.moqui.mobile:/dial?nonce=$nonce"))
            .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_SINGLE_TOP)
        try {
            opener(app, open)
        } catch (err: Exception) {
            Log.w(TAG, "The app did not open for the number: ${err.message}")
        }
        // Android may not let the app come to the front by itself: a notification then hands the number over on a tap
        Handler(Looper.getMainLooper()).postDelayed({ if (!ChatNotifications.foreground) handoff(app, open, number) }, HANDOFF_WAIT_MS)
        Log.i(TAG, "A number from the phone's dialer goes to the app's Dialer")
        return Connection.createCanceledConnection()
    }

    private fun handoff(context: Context, open: Intent, number: String) {
        ChatNotifications.ensureChannels(context)
        val tap = PendingIntent.getActivity(context, HANDOFF_ID, open, PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT)
        val notification = NotificationCompat.Builder(context, ChatRules.MISSED)
            .setSmallIcon(R.mipmap.ic_launcher)
            .setContentTitle(context.getString(R.string.dial_handoff_title))
            .setContentText(context.getString(R.string.dial_handoff_text, CallRules.dialShown(number)))
            .setPriority(NotificationCompat.PRIORITY_HIGH)
            .setCategory(NotificationCompat.CATEGORY_CALL)
            .setContentIntent(tap)
            .setAutoCancel(true)
            .setTimeoutAfter(CallRules.DIAL_LINK_MS)
            .build()
        try {
            context.getSystemService(NotificationManager::class.java)?.notify(HANDOFF_ID, notification)
        } catch (err: SecurityException) {
            Log.w(TAG, "The number's notification was not shown: ${err.message}")
        }
    }

    /** The app opened for the number: its notification goes. */
    fun handedOver(context: Context) {
        context.getSystemService(NotificationManager::class.java)?.cancel(HANDOFF_ID)
    }
}
