// This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License.

package org.moqui.mobile

import android.content.Context
import android.os.Build
import android.os.OutcomeReceiver
import android.os.PowerManager
import android.telecom.CallAudioState
import android.telecom.CallEndpoint
import android.telecom.CallEndpointException
import android.util.Log
import androidx.annotation.RequiresApi
import org.json.JSONArray
import org.json.JSONObject
import java.util.concurrent.Executors

/**
 * The call's audio route through Telecom (plans/Comms-Messaging.md §8.2 During, §8.3 item 5): the WebView plays the
 * call's media, and Telecom routes the phone's audio for its call ([CallConnection]). The earpiece for a voice call and
 * the speaker for a video call by themselves ([CallRules.defaultRoute]); a Bluetooth or wired headset takes the call
 * when it comes and gives it back when it goes ([CallRules.afterRoutesChanged]); the page's speaker button chooses
 * (`call_audio_route`, [request]), which holds for the rest of the call. Android 14+ uses `CallEndpoint`, older ones the
 * `CallAudioState` routes. A voice call on the earpiece turns the screen off at the ear (the proximity wake lock). The
 * page hears each change (`moqui.call.setHostAudio`, through Rust).
 */
object CallAudio {
    private const val TAG = "moqui-calls"
    private val executor = Executors.newSingleThreadExecutor()

    /** The route in use and the ones there are, as Telecom last said. */
    @Volatile var route: String? = null
        private set
    @Volatile var routes: List<String> = emptyList()
        private set
    /** The user chose a route in this call (the page's speaker button). */
    @Volatile private var chosen = false
    /** Android 14+: the endpoints Telecom gave, by route. */
    private val endpoints = HashMap<String, Any>()
    private var proximity: PowerManager.WakeLock? = null

    /** A new call in Telecom: no choice of the user's yet. */
    fun reset() {
        chosen = false
        route = null
        routes = emptyList()
        endpoints.clear()
    }

    /** What the page shows: `{route, routes}` as JSON. */
    @JvmStatic
    fun state(): String = JSONObject().put("route", route ?: JSONObject.NULL).put("routes", JSONArray(routes)).toString()

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

    /** Android 14+: the endpoints there are now. */
    @RequiresApi(34)
    fun endpointsChanged(connection: CallConnection, list: List<CallEndpoint>) {
        val before = routes
        endpoints.clear()
        for (endpoint in list) typeOf(endpoint.endpointType)?.let { endpoints[it] = endpoint }
        routes = CallRules.ROUTES.filter { it in endpoints }
        afterChange(connection, before)
    }

    /** Android 14+: the endpoint in use. */
    @RequiresApi(34)
    fun endpointChanged(connection: CallConnection, endpoint: CallEndpoint) {
        route = typeOf(endpoint.endpointType)
        changed(connection)
    }

    /** Before Android 14: Telecom's audio state. */
    @Suppress("DEPRECATION")
    fun audioStateChanged(connection: CallConnection, state: CallAudioState) {
        val before = routes
        routes = CallRules.ROUTES.filter { it in routeOf(state.supportedRouteMask) }
        route = routeOf(state.route).firstOrNull()
        afterChange(connection, before)
    }

    private fun afterChange(connection: CallConnection, before: List<String>) {
        val call = CallManager.call(connection.id)
        val video = call?.video ?: false
        val next = if (before.isEmpty()) CallRules.defaultRoute(video, routes, route, chosen)
        else CallRules.afterRoutesChanged(video, before, routes, route)
        if (next != null) {
            if (before.isNotEmpty() && (next == CallRules.BLUETOOTH || next == CallRules.WIRED)) chosen = false
            set(connection, next)
        }
        changed(connection)
    }

    /** The call's kind changed (voice to video and back): its default route, unless the user chose one. */
    fun kindChanged(connection: CallConnection, video: Boolean) {
        CallRules.defaultRoute(video, routes, route, chosen)?.let { set(connection, it) }
        changed(connection)
    }

    /** The page's speaker button: the user's route for the rest of the call. Whether it was asked of Telecom. */
    @JvmStatic
    fun request(name: String): Boolean {
        val connection = CallManager.activeConnection() ?: return false
        if (name !in routes) return false
        chosen = true
        set(connection, name)
        return true
    }

    private fun set(connection: CallConnection, name: String) {
        if (Build.VERSION.SDK_INT >= 34) {
            val endpoint = endpoints[name] as? CallEndpoint ?: return
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

    /** After any change: the proximity lock as the call is, and the page told. */
    fun changed(connection: CallConnection?) {
        val call = connection?.let { CallManager.call(it.id) }
        val context = CallManager.context()
        if (context != null) proximity(context, CallRules.proximityWanted(call?.joined == true && !call.held, call?.video ?: false, route))
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
}
