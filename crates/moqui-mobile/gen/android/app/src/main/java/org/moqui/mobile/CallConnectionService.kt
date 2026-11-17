// This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License.

package org.moqui.mobile

import android.net.Uri
import android.telecom.CallAudioState
import android.telecom.CallEndpoint
import android.telecom.Connection
import android.telecom.ConnectionRequest
import android.telecom.ConnectionService
import android.telecom.PhoneAccount
import android.telecom.PhoneAccountHandle
import android.telecom.TelecomManager
import android.util.Log

/**
 * The app's self-managed calls (android.telecom, [CallManager]): Telecom knows of a ringing, answered or joined call,
 * so the system gives it the audio focus, routes its audio ([CallAudio]), holds it for another call and gives a
 * headset's or a watch's answer, decline and hang-up, and asks the app to show the ring
 * ([CallConnection.onShowIncomingCallUi]). A call rung here is incoming; one the app joined by itself (started, joined
 * from a chat, moved to this phone) is placed as outgoing. The media is the WebView's, not Telecom's.
 */
class CallConnectionService : ConnectionService() {
    override fun onCreateIncomingConnection(account: PhoneAccountHandle?, request: ConnectionRequest?): Connection {
        val id = request?.extras?.getString(CallManager.EXTRA_CALL)
            ?: request?.extras?.getBundle(TelecomManager.EXTRA_INCOMING_CALL_EXTRAS)?.getString(CallManager.EXTRA_CALL)
        val call = CallManager.call(id)
        val connection = CallConnection(this, id ?: "")
        if (call != null) {
            connection.setCallerDisplayName(call.caller, TelecomManager.PRESENTATION_ALLOWED)
            // a neutral address: a car or a watch shows the caller's name, not a call id
            connection.setAddress(Uri.fromParts(PhoneAccount.SCHEME_SIP, "moqui", null), TelecomManager.PRESENTATION_ALLOWED)
            connection.setRinging()
        }
        CallManager.connected(id, connection)
        return connection
    }

    override fun onCreateIncomingConnectionFailed(account: PhoneAccountHandle?, request: ConnectionRequest?) {
        val id = request?.extras?.getString(CallManager.EXTRA_CALL)
            ?: request?.extras?.getBundle(TelecomManager.EXTRA_INCOMING_CALL_EXTRAS)?.getString(CallManager.EXTRA_CALL)
        Log.w(CallManager.TAG, "Telecom did not make the call $id (another call?); its ring is shown without it")
        CallManager.notConnected(this, id)
    }

    /** A call the app joined by itself ([CallManager.placeInTelecom]): active at once, its media is already up. */
    override fun onCreateOutgoingConnection(account: PhoneAccountHandle?, request: ConnectionRequest?): Connection {
        val id = request?.extras?.getString(CallManager.EXTRA_CALL)
            ?: request?.address?.schemeSpecificPart
        val call = CallManager.call(id)
        val connection = CallConnection(this, id ?: "")
        connection.setAddress(Uri.fromParts(PhoneAccount.SCHEME_SIP, "moqui", null), TelecomManager.PRESENTATION_ALLOWED)
        // the call's name (the room or the other people, the page's) for a car or a watch
        val name = call?.caller?.takeIf { it.isNotEmpty() } ?: service().getString(R.string.call_in_app)
        connection.setCallerDisplayName(name, TelecomManager.PRESENTATION_ALLOWED)
        connection.setActive()
        CallManager.connected(id, connection)
        return connection
    }

    private fun service(): ConnectionService = this

    override fun onCreateOutgoingConnectionFailed(account: PhoneAccountHandle?, request: ConnectionRequest?) {
        val id = request?.extras?.getString(CallManager.EXTRA_CALL) ?: request?.address?.schemeSpecificPart
        Log.w(CallManager.TAG, "Telecom did not take the call $id the app joined: its audio is the WebView's own")
    }
}

/** One call in Telecom, whose events go to [CallManager] and [CallAudio]. */
class CallConnection(private val service: ConnectionService, val id: String) : Connection() {
    init {
        connectionProperties = PROPERTY_SELF_MANAGED
        connectionCapabilities = CAPABILITY_HOLD or CAPABILITY_SUPPORT_HOLD or CAPABILITY_MUTE
        audioModeIsVoip = true
    }

    override fun onShowIncomingCallUi() {
        CallManager.showRing(service.applicationContext, id)
    }

    override fun onAnswer() {
        CallManager.answer(null, service.applicationContext, id)
    }

    override fun onAnswer(videoState: Int) {
        onAnswer()
    }

    override fun onReject() {
        CallManager.decline(service.applicationContext, id)
    }

    override fun onDisconnect() {
        CallManager.disconnected(service.applicationContext, id)
    }

    override fun onAbort() {
        CallManager.disconnected(service.applicationContext, id)
    }

    /** Telecom holds the call (another call, the user's hold on a watch or a car): the page puts it on hold. */
    override fun onHold() {
        CallManager.telecomHold(id, true)
    }

    /** Telecom takes it off hold: the page resumes it. */
    override fun onUnhold() {
        CallManager.telecomHold(id, false)
    }

    override fun onAvailableCallEndpointsChanged(availableEndpoints: List<CallEndpoint>) {
        CallAudio.endpointsChanged(this, availableEndpoints)
    }

    override fun onCallEndpointChanged(callEndpoint: CallEndpoint) {
        CallAudio.endpointChanged(this, callEndpoint)
    }

    /** Android 14+: Telecom's mute (a car, a watch): the page's microphone follows. */
    override fun onMuteStateChanged(isMuted: Boolean) {
        CallAudio.muteChanged(this, isMuted)
    }

    @Deprecated("Android 14+ uses onCallEndpointChanged")
    override fun onCallAudioStateChanged(state: CallAudioState) {
        if (android.os.Build.VERSION.SDK_INT < 34) CallAudio.audioStateChanged(this, state)
    }
}
