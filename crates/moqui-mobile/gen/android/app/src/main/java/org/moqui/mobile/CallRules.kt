// This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License.

package org.moqui.mobile

/**
 * The decisions of [CallManager] that need no Android, so they are unit-tested on the JVM (src/test, CallRulesTest).
 */
object CallRules {
    /** A ring with no time of its own rings this long. */
    const val DEFAULT_RING_MS = 45_000L

    /** An answered call whose page has not joined it by then is ended here (encrypted on an old WebView, offline, not
     *  this account's...), so the phone is not left "in a call". */
    const val JOIN_WATCHDOG_MS = 30_000L

    /** How long a call's end is remembered, for a `ringEnded` that comes before its ring. */
    const val ENDED_MEMORY_MS = 120_000L

    /**
     * When a ring received at [now] ends: from `ringSeconds`, the time left when the server sent it (so the phone's
     * clock does not count), else from the server's `expiresAt`, else after [DEFAULT_RING_MS]. At or before [now] it
     * is over already.
     */
    fun expiresAt(now: Long, ringSeconds: String?, expiresAt: String?): Long {
        ringSeconds?.toLongOrNull()?.takeIf { it >= 0 }?.let { return now + it * 1000 }
        expiresAt?.toLongOrNull()?.takeIf { it > 0 }?.let { return it }
        return now + DEFAULT_RING_MS
    }

    /**
     * Whether the server's `ringEnded` with [reason] ends the call here: a ringing one always; one answered here and
     * joined by the page only when the call ended; one answered here and not joined yet unless it was answered (an end
     * the server sends the user's other devices).
     */
    fun ringEndedEnds(answered: Boolean, joined: Boolean, reason: String): Boolean = when {
        !answered -> true
        joined -> reason == "ended"
        else -> reason != "answered"
    }

    /** Whether an answered call the page has not joined is given up at [now]. */
    fun watchdogEnds(answered: Boolean, joined: Boolean, answeredAt: Long, now: Long): Boolean =
        answered && !joined && now - answeredAt >= JOIN_WATCHDOG_MS

    /** Whether a link with [linkId] and [linkNonce] is the one made for the call [callId] answered here ([callNonce]). */
    fun ownsLink(linkId: String?, linkNonce: String?, callId: String, callNonce: String?, answered: Boolean): Boolean =
        answered && callNonce != null && linkNonce == callNonce && linkId == callId
}
