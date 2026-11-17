// This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License.

package org.moqui.mobile

import android.content.Context
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.os.OutcomeReceiver
import android.os.PowerManager
import android.telecom.CallAudioState
import android.telecom.CallEndpoint
import android.telecom.CallEndpointException
import android.util.Log
import androidx.annotation.RequiresApi
import org.json.JSONArray
import org.json.JSONObject
import java.util.concurrent.CountDownLatch
import java.util.concurrent.Executors
import java.util.concurrent.TimeUnit

/**
 * The call's audio route through Telecom (plans/Comms-Messaging.md §8.2 During, §8.3 item 5): the WebView plays the
 * call's media, and Telecom routes the phone's audio for its call ([CallConnection]). Each call has its own state (its
 * routes, its route, whether the user chose one), so a second ring does not reset the call the app is in; only the
 * call the app is in now ([CallManager.activeConnection]) is routed, told to the page and given the screen off at the
 * ear. A headset (Bluetooth or wired) takes the call when it is there or comes, and gives it back when it goes; else the
 * earpiece for a voice call and the speaker for a video call ([CallRules.defaultRoute]); the page's audio button
 * chooses ([request]) for the rest of that call. Android 14+ uses `CallEndpoint`, older ones the `CallAudioState`
 * routes. Telecom's mute (a car, a watch) reaches the page. Everything here runs on the main thread.
 */
object CallAudio {
    private const val TAG = "moqui-calls"
    private val executor = Executors.newSingleThreadExecutor()
    private val main = Handler(Looper.getMainLooper())

    /** One call's audio. */
    private class Audio {
        var route: String? = null
        var routes: List<String> = emptyList()
        var chosen = false
        val endpoints = HashMap<String, Any>()
        var muted: Boolean? = null
    }

    private val audios = HashMap<String, Audio>()
    private var proximity: PowerManager.WakeLock? = null

    private fun audioOf(id: String): Audio = audios.getOrPut(id) { Audio() }

    private fun active(): CallConnection? = CallManager.activeConnection()

    /** The call the app is in now: its route, for the tests and the page. */
    val route: String? get() = active()?.let { audios[it.id]?.route }
    val routes: List<String> get() = active()?.let { audios[it.id]?.routes } ?: emptyList()

    /** A call ended: its audio state goes. */
    fun forget(id: String) {
        audios.remove(id)
    }

    /** What the page shows: `{route, routes}` of the call the app is in, as JSON. */
    @JvmStatic
    fun state(): String {
        val audio = active()?.let { audios[it.id] }
        return JSONObject().put("route", audio?.route ?: JSONObject.NULL).put("routes", JSONArray(audio?.routes ?: emptyList<String>())).toString()
    }

    private fun typeOf(type: Int): String? = when (type) {
        CallEndpoint.TYPE_EARPIECE -> CallRules.EARPIECE
        CallEndpoint.TYPE_SPEAKER -> CallRules.SPEAKER
        CallEndpoint.TYPE_BLUETOOTH -> CallRules.BLUETOOTH
        CallEndpoint.TYPE_WIRED_HEADSET -> CallRules.WIRED
        else -> null
    }

    @Suppress("DEPRECATION")
    private fun routeOf(mask: Int): List<String> = buildList {
        if (mask and CallAudioState.ROUTE_EARPIECE != 0) add(CallRules.EARPIECE)
        if (mask and CallAudioState.ROUTE_SPEAKER != 0) add(CallRules.SPEAKER)
        if (mask and CallAudioState.ROUTE_WIRED_HEADSET != 0) add(CallRules.WIRED)
        if (mask and CallAudioState.ROUTE_BLUETOOTH != 0) add(CallRules.BLUETOOTH)
    }

    /** Android 14+: the endpoints a call has now. */
    @RequiresApi(34)
    fun endpointsChanged(connection: CallConnection, list: List<CallEndpoint>) = onMain {
        val audio = audioOf(connection.id)
        val before = audio.routes
        audio.endpoints.clear()
        for (endpoint in list) typeOf(endpoint.endpointType)?.let { audio.endpoints[it] = endpoint }
        audio.routes = CallRules.ROUTES.filter { it in audio.endpoints }
        afterChange(connection, audio, before)
    }

    /** Android 14+: the endpoint a call uses. */
    @RequiresApi(34)
    fun endpointChanged(connection: CallConnection, endpoint: CallEndpoint) = onMain {
        audioOf(connection.id).route = typeOf(endpoint.endpointType)
        changed(connection)
    }

    /** Android 14+: Telecom's mute of a call. */
    fun muteChanged(connection: CallConnection, muted: Boolean) = onMain {
        val audio = audioOf(connection.id)
        if (audio.muted == muted) return@onMain
        audio.muted = muted
        if (connection === active()) Native.tellPage("mute", muted.toString())
    }

    /** Before Android 14: Telecom's audio state of a call. */
    @Suppress("DEPRECATION")
    fun audioStateChanged(connection: CallConnection, state: CallAudioState) = onMain {
        val audio = audioOf(connection.id)
        val before = audio.routes
        audio.routes = CallRules.ROUTES.filter { it in routeOf(state.supportedRouteMask) }
        audio.route = routeOf(state.route).firstOrNull()
        afterChange(connection, audio, before)
        if (audio.muted != state.isMuted) {
            val first = audio.muted == null
            audio.muted = state.isMuted
            if (!first && connection === active()) Native.tellPage("mute", state.isMuted.toString())
        }
    }

    private fun afterChange(connection: CallConnection, audio: Audio, before: List<String>) {
        if (connection === active()) {
            val video = CallManager.call(connection.id)?.video ?: false
            val next = if (before.isEmpty()) CallRules.defaultRoute(video, audio.routes, audio.route, audio.chosen)
            else CallRules.afterRoutesChanged(video, before, audio.routes, audio.route)
            if (next != null) {
                if (before.isNotEmpty() && (next == CallRules.BLUETOOTH || next == CallRules.WIRED)) audio.chosen = false
                set(connection, audio, next)
            }
        }
        changed(connection)
    }

    /** A call's kind changed (voice to video and back), or it became the call the app is in: its default route. */
    fun kindChanged(connection: CallConnection, video: Boolean) = onMain {
        if (connection !== active()) return@onMain
        val audio = audioOf(connection.id)
        CallRules.defaultRoute(video, audio.routes, audio.route, audio.chosen)?.let { set(connection, audio, it) }
        changed(connection)
    }

    /**
     * The page's audio button (Rust's command thread): the user's route for the rest of the call the app is in, set on
     * the main thread. Whether it was asked of Telecom.
     */
    @JvmStatic
    fun request(name: String): Boolean {
        if (Looper.myLooper() == Looper.getMainLooper()) return requestNow(name)
        var asked = false
        val done = CountDownLatch(1)
        main.post {
            try {
                asked = requestNow(name)
            } finally {
                done.countDown()
            }
        }
        done.await(2, TimeUnit.SECONDS)
        return asked
    }

    private fun requestNow(name: String): Boolean {
        val connection = active() ?: return false
        val audio = audioOf(connection.id)
        if (name !in audio.routes) return false
        audio.chosen = true
        set(connection, audio, name)
        return true
    }

    private fun set(connection: CallConnection, audio: Audio, name: String) {
        if (Build.VERSION.SDK_INT >= 34) {
            val endpoint = audio.endpoints[name] as? CallEndpoint ?: return
            connection.requestCallEndpointChange(endpoint, executor, object : OutcomeReceiver<Void, CallEndpointException> {
                override fun onResult(result: Void?) {}
                override fun onError(error: CallEndpointException) {
                    Log.w(TAG, "The call's audio did not go to $name: ${error.message}")
                }
            })
        } else {
            @Suppress("DEPRECATION")
            val mask = when (name) {
                CallRules.EARPIECE -> CallAudioState.ROUTE_EARPIECE
                CallRules.SPEAKER -> CallAudioState.ROUTE_SPEAKER
                CallRules.WIRED -> CallAudioState.ROUTE_WIRED_HEADSET
                CallRules.BLUETOOTH -> CallAudioState.ROUTE_BLUETOOTH
                else -> return
            }
            @Suppress("DEPRECATION")
            connection.setAudioRoute(mask)
        }
    }

    /**
     * After any change: the proximity lock as the call the app is in is (a voice call on the earpiece, its call screen
     * showing in the app in front), and the page told of that call's audio. A change of another call is kept for later.
     */
    fun changed(connection: CallConnection?) {
        val now = active()
        if (connection != null && connection !== now) return
        val call = now?.let { CallManager.call(it.id) }
        val context = CallManager.context()
        val inFront = CallManager.screenShown && ChatNotifications.foreground
        if (context != null) proximity(context, CallRules.proximityWanted(call != null, call?.video ?: false, route, inFront))
        Native.tellPage("audio", state())
    }

    /** The screen off at the ear: held while [on]. */
    fun proximity(context: Context, on: Boolean) {
        val power = context.getSystemService(PowerManager::class.java) ?: return
        if (on) {
            if (proximity?.isHeld == true) return
            if (!power.isWakeLockLevelSupported(PowerManager.PROXIMITY_SCREEN_OFF_WAKE_LOCK)) return
            proximity = power.newWakeLock(PowerManager.PROXIMITY_SCREEN_OFF_WAKE_LOCK, "moqui:call-proximity").also {
                it.setReferenceCounted(false)
                it.acquire(4 * 60 * 60 * 1000L)
            }
        } else {
            proximity?.let { if (it.isHeld) it.release(PowerManager.RELEASE_FLAG_WAIT_FOR_NO_PROXIMITY) }
            proximity = null
        }
    }

    /** Whether the screen is off at the ear now (the instrumented tests). */
    fun proximityHeld(): Boolean = proximity?.isHeld == true

    private fun onMain(task: () -> Unit) {
        if (Looper.myLooper() == Looper.getMainLooper()) task() else main.post(task)
    }
}
