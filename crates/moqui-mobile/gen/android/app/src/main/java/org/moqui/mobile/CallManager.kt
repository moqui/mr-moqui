// This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License.

package org.moqui.mobile

import android.app.Activity
import android.content.ComponentName
import android.content.Context
import android.content.Intent
import android.net.Uri
import android.os.Build
import android.os.Bundle
import android.os.Handler
import android.os.Looper
import android.telecom.Connection
import android.telecom.DisconnectCause
import android.telecom.PhoneAccount
import android.telecom.PhoneAccountHandle
import android.telecom.TelecomManager
import android.util.Log
import android.webkit.WebView
import java.lang.ref.WeakReference

/**
 * The incoming call on a locked phone (plans/Mobile-Desktop.md MD6, MT2): a ring from the server's push (FCM, or the
 * debug build's injection) shown by native code, with no WebView, and the call's media in the WebView once answered.
 *
 * - A ring becomes a self-managed Telecom call ([CallConnectionService]); Telecom asks for its incoming-call UI, which
 *   is the `phoneCall` foreground service's notification ([CallService]) with a full-screen intent to
 *   [IncomingCallActivity] (a heads-up notification where Android 14+ does not allow the full-screen one).
 * - It rings until the server's `ringEnded`, its `expiresAt`, or the user's Answer or Decline.
 * - Answer opens [MainActivity] over the lock screen with `org.moqui.mobile:/call?callSessionId=...`, which Rust shows
 *   in the shell's call screen, which answers and joins (WebrootCall.js); the Telecom call and the foreground service
 *   last until the page says the call is over ([pageCallState], from Rust).
 * - Decline goes to the server from Rust ([Native.decline]) with the device key, with no WebView.
 *
 * Everything here runs on the main thread; the calls from other threads are posted to it.
 */
object CallManager {
    const val TAG = "moqui-calls"
    const val EXTRA_CALL = "org.moqui.mobile.callSessionId"
    private const val ACCOUNT_ID = "moqui-calls"
    /** A ring with no `expiresAt` rings this long. */
    private const val DEFAULT_RING_MS = 45_000L
    private val ID = Regex("^[A-Za-z0-9_-]{1,64}$")

    enum class State { RINGING, ANSWERED }

    class Call(
        val id: String,
        val caller: String,
        val title: String,
        val video: Boolean,
        val expiresAt: Long,
        var state: State,
    ) {
        var connection: CallConnection? = null
        var shown = false
    }

    private val main = Handler(Looper.getMainLooper())
    private val calls = LinkedHashMap<String, Call>()
    /** Declines still being sent: the foreground service stays until they are. */
    private var sending = 0
    private var ringScreen: WeakReference<IncomingCallActivity>? = null
    private var appScreen: WeakReference<MainActivity>? = null
    /** The application context, once any part of the app ran. */
    @Volatile private var appContext: Context? = null

    fun know(context: Context) {
        appContext = context.applicationContext
    }

    private fun onMain(task: () -> Unit) {
        if (Looper.myLooper() == Looper.getMainLooper()) task() else main.post(task)
    }

    /** A push's data (FCM's, or the debug injection's). */
    fun onPush(context: Context, data: Map<String, String>) {
        val app = context.applicationContext
        know(app)
        when (data["kind"]) {
            "ring" -> onMain { ring(app, data) }
            "ringEnded" -> onMain { ringEnded(app, data["callSessionId"] ?: "", data["reason"] ?: "ended") }
            else -> Log.i(TAG, "A push of kind ${data["kind"]} is not shown by the native code yet")
        }
    }

    fun call(id: String?): Call? = id?.let { calls[it] }

    /** Whether a call is ringing or answered here: backing out of the app then keeps the process. */
    fun inCall(): Boolean = calls.isNotEmpty()

    fun ringing(): Call? = calls.values.firstOrNull { it.state == State.RINGING }

    private fun ring(context: Context, data: Map<String, String>) {
        val id = data["callSessionId"]?.takeIf { ID.matches(it) } ?: run {
            Log.w(TAG, "A ring with no call id")
            return
        }
        val now = System.currentTimeMillis()
        val expiresAt = data["expiresAt"]?.toLongOrNull()?.takeIf { it > 0 } ?: (now + DEFAULT_RING_MS)
        if (expiresAt <= now) {
            Log.i(TAG, "The ring of $id was over before it came")
            return
        }
        if (calls.containsKey(id)) return
        val caller = data["caller"]?.takeIf { it.isNotBlank() }?.take(200) ?: "MR Moqui"
        val call = Call(id, caller, data["title"]?.take(200) ?: "", data["video"] == "true", expiresAt, State.RINGING)
        calls[id] = call
        main.postDelayed({ ringEnded(context, id, "timedOut") }, expiresAt - now)
        Log.i(TAG, "Ringing for $id")
        if (!addToTelecom(context, call)) showRing(context, id)
        // Telecom asks for the ring's UI (onShowIncomingCallUi); if it has not soon, it is shown all the same
        main.postDelayed({ if (calls[id]?.shown == false) showRing(context, id) }, 3_000)
    }

    /** The app's self-managed phone account, registered (again: it is idempotent) before a call. */
    private fun accountHandle(context: Context): PhoneAccountHandle? {
        val telecom = context.getSystemService(TelecomManager::class.java) ?: return null
        val handle = PhoneAccountHandle(ComponentName(context, CallConnectionService::class.java), ACCOUNT_ID)
        return try {
            telecom.registerPhoneAccount(
                PhoneAccount.builder(handle, "MR Moqui")
                    .setCapabilities(PhoneAccount.CAPABILITY_SELF_MANAGED)
                    .addSupportedUriScheme(PhoneAccount.SCHEME_SIP)
                    .build()
            )
            handle
        } catch (err: Exception) {
            Log.w(TAG, "No Telecom phone account: ${err.message}")
            null
        }
    }

    private fun addToTelecom(context: Context, call: Call): Boolean {
        val handle = accountHandle(context) ?: return false
        val telecom = context.getSystemService(TelecomManager::class.java) ?: return false
        return try {
            val extras = Bundle().apply {
                putParcelable(TelecomManager.EXTRA_PHONE_ACCOUNT_HANDLE, handle)
                putBundle(TelecomManager.EXTRA_INCOMING_CALL_EXTRAS, Bundle().apply { putString(EXTRA_CALL, call.id) })
            }
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O && !telecom.isIncomingCallPermitted(handle)) {
                Log.i(TAG, "Telecom does not permit an incoming call now (another call); the ring is shown without it")
                return false
            }
            telecom.addNewIncomingCall(handle, extras)
            true
        } catch (err: Exception) {
            Log.w(TAG, "Telecom did not take the call: ${err.message}")
            false
        }
    }

    /** Telecom made the call's connection ([CallConnectionService]). */
    fun connected(id: String?, connection: CallConnection) {
        val call = call(id)
        if (call == null) {
            connection.setDisconnected(DisconnectCause(DisconnectCause.CANCELED))
            connection.destroy()
            return
        }
        call.connection = connection
    }

    /** Telecom could not make it: the ring is shown without it. */
    fun notConnected(context: Context, id: String?) {
        if (id != null) onMain { showRing(context.applicationContext, id) }
    }

    /** The ring's UI: the foreground service and its notification, which opens the ring screen. */
    fun showRing(context: Context, id: String) {
        val call = calls[id] ?: return
        if (call.shown) return
        call.shown = true
        CallService.refresh(context)
    }

    /** Answer, from the ring screen, its notification, or Telecom (a headset, a watch). */
    fun answer(activity: Activity?, context: Context, id: String) {
        val call = calls[id] ?: run {
            Log.i(TAG, "Answer of $id, which no longer rings")
            return
        }
        call.state = State.ANSWERED
        call.connection?.setActive()
        closeRingScreen()
        CallService.refresh(context)
        val open = Intent(context, MainActivity::class.java)
            .setAction(Intent.ACTION_VIEW)
            .setData(Uri.parse("org.moqui.mobile:/call?callSessionId=${call.id}&video=${call.video}"))
            .putExtra(EXTRA_CALL, call.id)
            .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_SINGLE_TOP)
        try {
            (activity ?: context).startActivity(open)
        } catch (err: Exception) {
            Log.e(TAG, "The app did not open for the call $id", err)
        }
        Log.i(TAG, "Answered $id; the app joins it")
    }

    /** Decline, from the ring screen, its notification, or Telecom: the server is told from Rust. */
    fun decline(context: Context, id: String) {
        val app = context.applicationContext
        val call = calls.remove(id) ?: return
        end(call, DisconnectCause.REJECTED)
        sending++
        CallService.refresh(app)
        Native.work {
            val told = if (Native.ensure(app)) Native.decline(Native.dataDir(app), id) else "the app's library did not load"
            Log.i(TAG, "Declined $id: $told")
            onMain {
                sending--
                CallService.refresh(app)
            }
        }
    }

    /** The server's `ringEnded`, or the ring's end: a ringing call stops (one answered here goes on until the page ends
     *  it, unless the call ended). */
    fun ringEnded(context: Context, id: String, reason: String) {
        val call = calls[id] ?: return
        if (call.state == State.ANSWERED && reason != "ended") return
        calls.remove(id)
        val cause = when (reason) {
            "answered" -> if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) DisconnectCause.ANSWERED_ELSEWHERE else DisconnectCause.OTHER
            "declined" -> DisconnectCause.REJECTED
            "timedOut" -> DisconnectCause.MISSED
            else -> DisconnectCause.REMOTE
        }
        end(call, cause)
        Log.i(TAG, "The ring of $id ended: $reason")
        CallService.refresh(context.applicationContext)
    }

    /** The shell's call joined or over (Rust's `call_state`, from WebrootCall.js). */
    @JvmStatic
    fun pageCallState(id: String, active: Boolean) {
        onMain {
            val context = appContext ?: return@onMain
            val call = calls[id]
            if (active) {
                if (call == null) {
                    // a call joined in the app (not rung here): the foreground service keeps it while the screen is off
                    calls[id] = Call(id, "", "", false, 0, State.ANSWERED).also { it.shown = true }
                } else if (call.state == State.RINGING) {
                    call.state = State.ANSWERED
                    call.connection?.setActive()
                    closeRingScreen()
                }
                Log.i(TAG, "The app is in the call $id")
            } else if (call != null && call.state == State.ANSWERED) {
                calls.remove(id)
                end(call, DisconnectCause.LOCAL)
                Log.i(TAG, "The app left the call $id")
            }
            CallService.refresh(context)
            if (calls.isEmpty()) appScreen?.get()?.showOverLock(false)
        }
    }

    /** Telecom ended the call (a headset's hang-up): the native side lets it go. */
    fun disconnected(context: Context, id: String) {
        onMain {
            val call = calls.remove(id) ?: return@onMain
            end(call, DisconnectCause.LOCAL)
            CallService.refresh(context.applicationContext)
        }
    }

    private fun end(call: Call, cause: Int) {
        call.connection?.let {
            it.setDisconnected(DisconnectCause(cause))
            it.destroy()
        }
        call.connection = null
        if (ringing() == null) closeRingScreen()
    }

    fun ringScreenShown(activity: IncomingCallActivity?) {
        ringScreen = activity?.let { WeakReference(it) }
    }

    fun appScreenShown(activity: MainActivity?) {
        appScreen = activity?.let { WeakReference(it) }
    }

    private fun closeRingScreen() {
        ringScreen?.get()?.finish()
        ringScreen = null
    }

    /** Whether the foreground service still has work: a call, or a decline being sent. */
    fun busy(): Boolean = calls.isNotEmpty() || sending > 0

    fun calls(): Collection<Call> = calls.values

    /** The WebView's major version, or 0 when it is not known. */
    fun webViewMajor(): Int = try {
        WebView.getCurrentWebViewPackage()?.versionName?.substringBefore('.')?.toIntOrNull() ?: 0
    } catch (err: Exception) {
        0
    }

    /** Encrypted calls need RTCRtpScriptTransform: Android System WebView 141 or later (they fail closed below it). */
    const val ENCRYPTED_CALLS_WEBVIEW = 141
}
