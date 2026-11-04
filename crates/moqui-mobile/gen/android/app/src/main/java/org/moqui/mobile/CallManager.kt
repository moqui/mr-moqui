// This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License.

package org.moqui.mobile

import android.app.Activity
import android.app.KeyguardManager
import android.content.ComponentName
import android.content.Context
import android.content.Intent
import android.net.Uri
import android.os.Build
import android.os.Bundle
import android.os.Handler
import android.os.Looper
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
 * - A ring is for the account its push names (`pushId`, [Native.ringAccount]); one that is no account's here is
 *   dropped. It becomes a self-managed Telecom call ([CallConnectionService]); Telecom asks for its incoming-call UI,
 *   which is the `phoneCall` foreground service's notification ([CallService]) with a full-screen intent to
 *   [IncomingCallActivity] (a heads-up notification where Android 14+ does not allow the full-screen one).
 * - It rings until the server's `ringEnded`, its time is up, or the user's Answer or Decline.
 * - Answer opens [MainActivity] over the lock screen with a link whose nonce Rust made for this Answer
 *   ([Native.callLink]), which Rust takes once and shows in the shell's call screen of the ring's account; the page
 *   answers and joins (WebrootCall.js) and says so ([pageCallState]). The app shows over the lock screen only for a
 *   call answered here, and only its call screen; a call the page has not joined in [CallRules.JOIN_WATCHDOG_MS] is
 *   given up.
 * - Decline goes to the ring's account's server from Rust ([Native.decline]) with the device key, with no WebView.
 *
 * Everything here runs on the main thread; the calls from other threads are posted to it.
 */
object CallManager {
    const val TAG = "moqui-calls"
    const val EXTRA_CALL = "org.moqui.mobile.callSessionId"
    private const val ACCOUNT_ID = "moqui-calls"
    private val ID = Regex("^[A-Za-z0-9_-]{1,64}$")

    enum class State { RINGING, ANSWERED }

    class Call(
        val id: String,
        /** The account (Rust's id) the ring is for; empty for a call joined in the app, not rung here. */
        val account: String,
        val caller: String,
        val title: String,
        val video: Boolean,
        val expiresAt: Long,
        var state: State,
    ) {
        var connection: CallConnection? = null
        var shown = false
        /** The nonce of the Answer's link: the app shows over the lock screen for this call only. */
        var answerNonce: String? = null
        var answeredAt = 0L
        /** The page joined its media. */
        var joined = false
    }

    private val main = Handler(Looper.getMainLooper())
    private val calls = LinkedHashMap<String, Call>()
    /** Calls whose end came (a `ringEnded` before its ring), with when. */
    private val ended = LinkedHashMap<String, Long>()
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
            "ring" -> ring(app, data)
            "ringEnded" -> onMain { ringEnded(app, data["callSessionId"] ?: "", data["reason"] ?: "ended") }
            else -> Log.i(TAG, "A push of kind ${data["kind"]} is not shown by the native code yet")
        }
    }

    fun call(id: String?): Call? = id?.let { calls[it] }

    /** Whether a call is ringing or answered here: backing out of the app then keeps the process. */
    fun inCall(): Boolean = calls.isNotEmpty()

    fun ringing(): Call? = calls.values.firstOrNull { it.state == State.RINGING }

    /** A ring: its account found by Rust (off the main thread), then shown. */
    private fun ring(context: Context, data: Map<String, String>) {
        val received = System.currentTimeMillis()
        val id = data["callSessionId"]?.takeIf { ID.matches(it) } ?: run {
            Log.w(TAG, "A ring with no call id")
            return
        }
        val pushId = data["pushId"] ?: ""
        Native.work {
            val account = if (Native.ensure(context)) Native.ringAccount(Native.dataDir(context), pushId) else null
            onMain {
                if (account == null) Log.w(TAG, "A ring of $id that is no account's here (push $pushId): dropped")
                else rang(context, data, id, account, received)
            }
        }
    }

    private fun rang(context: Context, data: Map<String, String>, id: String, account: String, received: Long) {
        val now = System.currentTimeMillis()
        ended.entries.removeAll { now - it.value > CallRules.ENDED_MEMORY_MS }
        if (ended.containsKey(id)) {
            Log.i(TAG, "The ring of $id had ended before it came")
            return
        }
        val expiresAt = CallRules.expiresAt(received, data["ringSeconds"], data["expiresAt"])
        if (expiresAt <= now) {
            Log.i(TAG, "The ring of $id was over before it came")
            return
        }
        if (calls.containsKey(id)) return
        val caller = data["caller"]?.takeIf { it.isNotBlank() }?.take(200) ?: "MR Moqui"
        val call = Call(id, account, caller, data["title"]?.take(200) ?: "", data["video"] == "true", expiresAt, State.RINGING)
        calls[id] = call
        // timers check the call is still this one: a later ring of the same id has its own
        main.postDelayed({ if (calls[id] === call) ringEnded(context, id, "timedOut") }, expiresAt - now)
        Log.i(TAG, "Ringing for $id")
        if (!addToTelecom(context, call)) showRing(context, id)
        // Telecom asks for the ring's UI (onShowIncomingCallUi); if it has not soon, it is shown all the same
        main.postDelayed({ if (calls[id] === call && !call.shown) showRing(context, id) }, 3_000)
    }

    /** The app's self-managed phone account, registered (again: it is idempotent) before a call. */
    private fun accountHandle(context: Context): PhoneAccountHandle? {
        val telecom = context.getSystemService(TelecomManager::class.java) ?: return null
        val handle = PhoneAccountHandle(ComponentName(context, CallConnectionService::class.java), ACCOUNT_ID)
        return try {
            @Suppress("DEPRECATION") // CallControl (Core-Telecom) at API 34+: with the Rust call client
            val selfManaged = PhoneAccount.CAPABILITY_SELF_MANAGED
            telecom.registerPhoneAccount(
                PhoneAccount.builder(handle, "MR Moqui")
                    .setCapabilities(selfManaged)
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
            if (!telecom.isIncomingCallPermitted(handle)) {
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
        val call = calls[id]
        if (call == null || call.state != State.RINGING) {
            Log.i(TAG, "Answer of $id, which no longer rings")
            return
        }
        val app = context.applicationContext
        val nonce = if (Native.ensure(app)) Native.callLink(call.id, call.account, call.video, false) else null
        if (nonce == null) {
            Log.e(TAG, "No link for the answer of $id: it is not opened")
            return
        }
        call.state = State.ANSWERED
        call.answerNonce = nonce
        call.answeredAt = System.currentTimeMillis()
        call.connection?.setActive()
        closeRingScreen()
        CallService.refresh(app)
        main.postDelayed({ watchdog(app, call) }, CallRules.JOIN_WATCHDOG_MS)
        val open = Intent(context, MainActivity::class.java)
            .setAction(Intent.ACTION_VIEW)
            .setData(Uri.parse("org.moqui.mobile:/call?callSessionId=${call.id}&nonce=$nonce"))
            .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_SINGLE_TOP)
        try {
            (activity ?: context).startActivity(open)
        } catch (err: Exception) {
            Log.e(TAG, "The app did not open for the call $id", err)
        }
        Log.i(TAG, "Answered $id; the app joins it")
    }

    /** An answered call the page has not joined by now is given up: the phone is not left in a call. */
    private fun watchdog(context: Context, call: Call) {
        if (calls[call.id] !== call) return
        if (!CallRules.watchdogEnds(call.state == State.ANSWERED, call.joined, call.answeredAt, System.currentTimeMillis())) return
        Log.w(TAG, "The page did not join ${call.id}: the call here ends")
        calls.remove(call.id)
        end(call, DisconnectCause.LOCAL)
        changed(context)
    }

    /** Decline, from the ring screen, its notification, or Telecom: the ring's account's server is told from Rust. */
    fun decline(context: Context, id: String) {
        val app = context.applicationContext
        val call = calls[id]?.takeIf { it.state == State.RINGING } ?: return
        calls.remove(id)
        end(call, DisconnectCause.REJECTED)
        sending++
        changed(app)
        Native.work {
            val told = if (Native.ensure(app)) Native.decline(Native.dataDir(app), id, call.account)
            else "the app's library did not load"
            Log.i(TAG, "Declined $id: $told")
            onMain {
                sending--
                CallService.refresh(app)
            }
        }
    }

    /** The server's `ringEnded`, or the ring's end ([CallRules.ringEndedEnds]); one for a call not here yet is
     *  remembered, so its ring does not start after it. */
    fun ringEnded(context: Context, id: String, reason: String) {
        if (!ID.matches(id)) return
        val call = calls[id]
        if (call == null) {
            ended[id] = System.currentTimeMillis()
            return
        }
        if (!CallRules.ringEndedEnds(call.state == State.ANSWERED, call.joined, reason)) return
        calls.remove(id)
        val cause = when (reason) {
            "answered" -> if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) DisconnectCause.ANSWERED_ELSEWHERE else DisconnectCause.OTHER
            "declined" -> DisconnectCause.REJECTED
            "timedOut" -> DisconnectCause.MISSED
            else -> DisconnectCause.REMOTE
        }
        end(call, cause)
        if (call.state == State.ANSWERED && call.joined) hangUpPage(call.id)
        Log.i(TAG, "The ring of $id ended: $reason")
        changed(context.applicationContext)
    }

    /**
     * What the shell says of a call (Rust's `call_state`, from WebrootCall.js): `joined`, `left`, `screen`, `offScreen`
     * (with an empty id: the shell left for a server page).
     */
    @JvmStatic
    fun pageCallState(id: String, state: String) {
        onMain {
            val context = appContext ?: return@onMain
            val call = calls[id]
            when (state) {
                "joined" -> {
                    if (call == null) {
                        // a call joined in the app (not rung here): the foreground service keeps it while the screen is off
                        calls[id] = Call(id, "", "", "", false, 0, State.ANSWERED).also {
                            it.shown = true
                            it.joined = true
                        }
                    } else {
                        if (call.state == State.RINGING) {
                            // answered in the app itself, not on the ring screen
                            call.state = State.ANSWERED
                            call.connection?.setActive()
                            closeRingScreen()
                        }
                        call.joined = true
                    }
                    Log.i(TAG, "The app is in the call $id")
                }
                "left" -> if (call != null && call.state == State.ANSWERED) {
                    calls.remove(id)
                    end(call, DisconnectCause.LOCAL)
                    Log.i(TAG, "The app left the call $id")
                }
                "screen" -> if (call != null && call.answerNonce != null && call.state == State.ANSWERED) {
                    appScreen?.get()?.showOverLock(true)
                }
                "offScreen" -> leaveLockScreen()
            }
            changed(context)
        }
    }

    /** Telecom ended the call (a headset's or a car's hang-up): an answered one is left by the page too. */
    fun disconnected(context: Context, id: String) {
        onMain {
            val call = calls.remove(id) ?: return@onMain
            end(call, DisconnectCause.LOCAL)
            if (call.state == State.ANSWERED) hangUpPage(id)
            changed(context.applicationContext)
        }
    }

    /** The ongoing call notification's Hang up: the page leaves the call (and says so), or it ends here. */
    fun hangUp(context: Context, id: String) {
        val call = calls[id] ?: return
        if (call.joined) {
            hangUpPage(id)
        } else {
            calls.remove(id)
            end(call, DisconnectCause.LOCAL)
            changed(context.applicationContext)
        }
    }

    private fun hangUpPage(id: String) {
        val app = appContext ?: return
        Native.work { if (Native.ensure(app)) Native.hangUp(id) }
    }

    private fun end(call: Call, cause: Int) {
        call.connection?.let {
            it.setDisconnected(DisconnectCause(cause))
            it.destroy()
        }
        call.connection = null
        if (ringing() == null) closeRingScreen()
    }

    /** After any change: the service as the calls are, and no showing over the lock screen without a call answered
     *  here. */
    private fun changed(context: Context) {
        CallService.refresh(context)
        if (calls.values.none { it.state == State.ANSWERED && it.answerNonce != null }) appScreen?.get()?.showOverLock(false)
    }

    /** Over the lock screen the app shows the call's screen only: leaving it shows the lock screen, which asks for the
     *  unlock. */
    private fun leaveLockScreen() {
        val activity = appScreen?.get() ?: return
        val keyguard = activity.getSystemService(KeyguardManager::class.java)
        if (keyguard?.isKeyguardLocked != true) return
        activity.showOverLock(false)
        keyguard.requestDismissKeyguard(activity, null)
    }

    /** Whether `data` is the link made for a call answered here: only then does the app show over the lock screen. */
    fun ownsLink(data: Uri?): Boolean {
        if (data == null || data.scheme != "org.moqui.mobile" || data.path != "/call") return false
        val id = data.getQueryParameter("callSessionId")
        val call = call(id) ?: return false
        return CallRules.ownsLink(id, data.getQueryParameter("nonce"), call.id, call.answerNonce, call.state == State.ANSWERED)
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

    /** [busy] as last asked on the main thread, for another thread. */
    @Volatile private var lastBusy = false

    /** Whether the foreground service still has work: a call, or a decline being sent. */
    fun busy(): Boolean = (calls.isNotEmpty() || sending > 0).also { lastBusy = it }

    /** [busy], for Rust's thread (the activity's end: the process stays while a call or a decline is under way). */
    @JvmStatic
    fun busyNow(): Boolean = lastBusy

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
