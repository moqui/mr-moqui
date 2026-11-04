// This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License.

package org.moqui.mobile

import android.net.Uri
import android.telecom.Connection
import android.telecom.ConnectionRequest
import android.telecom.ConnectionService
import android.telecom.PhoneAccountHandle
import android.telecom.TelecomManager
import android.util.Log

/**
 * The app's self-managed calls (android.telecom, [CallManager]): Telecom knows of a ringing or answered call, so the
 * system gives it the audio focus and a headset's or a watch's answer, decline and hang-up, and asks the app to show
 * the ring ([CallConnection.onShowIncomingCallUi]). The media is the WebView's, not Telecom's.
 */
class CallConnectionService : ConnectionService() {
    override fun onCreateIncomingConnection(account: PhoneAccountHandle?, request: ConnectionRequest?): Connection {
        val id = request?.extras?.getString(CallManager.EXTRA_CALL)
            ?: request?.extras?.getBundle(TelecomManager.EXTRA_INCOMING_CALL_EXTRAS)?.getString(CallManager.EXTRA_CALL)
        val call = CallManager.call(id)
        val connection = CallConnection(this, id ?: "")
        connection.connectionProperties = Connection.PROPERTY_SELF_MANAGED
        connection.audioModeIsVoip = true
        if (call != null) {
            connection.setCallerDisplayName(call.caller, TelecomManager.PRESENTATION_ALLOWED)
            connection.setAddress(Uri.fromParts("sip", call.id, null), TelecomManager.PRESENTATION_ALLOWED)
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
}

/** One call in Telecom, whose events go to [CallManager]. */
class CallConnection(private val service: ConnectionService, val id: String) : Connection() {
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
}
