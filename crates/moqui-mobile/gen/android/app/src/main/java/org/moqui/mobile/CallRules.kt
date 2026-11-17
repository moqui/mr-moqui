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

    // ---- audio, the screen at the ear, picture-in-picture, the service (plans/Comms-Messaging.md §8.2, §8.3 item 5) ----

    const val EARPIECE = "earpiece"
    const val SPEAKER = "speaker"
    const val BLUETOOTH = "bluetooth"
    const val WIRED = "wired"

    /** The routes in the order the page's menu lists them. */
    val ROUTES = listOf(EARPIECE, SPEAKER, WIRED, BLUETOOTH)

    /**
     * The route a call goes to by itself, when the user has not chosen one in it: a headset (Bluetooth or wired) that
     * Telecom took stays, and one that is there takes the call (at a call's start Telecom may name the earpiece before
     * Bluetooth's audio is up); else the speaker for a video call, the earpiece for a voice call (the speaker where the
     * phone has no earpiece). Null: no change.
     */
    fun defaultRoute(video: Boolean, available: Collection<String>, current: String?, chosen: Boolean): String? {
        if (chosen) return null
        if (current == BLUETOOTH || current == WIRED) return null
        val headset = listOf(BLUETOOTH, WIRED).firstOrNull { it in available }
        val wanted = headset ?: if (video) SPEAKER else if (EARPIECE in available) EARPIECE else SPEAKER
        return wanted.takeIf { it != current && it in available }
    }

    /**
     * After a headset comes or goes: a new headset takes the call at once (Telecom's own), and when the one in use goes
     * the call falls back to the default of its kind, whatever the user chose before.
     */
    fun afterRoutesChanged(video: Boolean, before: Collection<String>, now: Collection<String>, current: String?): String? {
        val added = now.filter { it !in before && (it == BLUETOOTH || it == WIRED) }
        if (added.isNotEmpty()) return added.first().takeIf { it != current }
        if (current != null && current !in now) return defaultRoute(video, now, null, false)
        return null
    }

    /** The screen goes off at the ear: a voice call on the earpiece, with its call screen showing in the app in front. */
    fun proximityWanted(inCall: Boolean, video: Boolean, route: String?, screenInFront: Boolean): Boolean =
        inCall && !video && route == EARPIECE && screenInFront

    /** A video call, in the app and on its call screen, goes to picture-in-picture when the user leaves the app. */
    fun pipWanted(inCall: Boolean, video: Boolean, onCallScreen: Boolean): Boolean = inCall && video && onCallScreen

    /**
     * The foreground service's types: `phoneCall` always; `microphone` once the app is in a call with the microphone
     * allowed, and `camera` too for a video call with the camera allowed, so the call keeps its media when the app is
     * left (a ring alone has neither: Android does not let a service started from the background have them).
     */
    fun serviceTypes(inCall: Boolean, video: Boolean, micAllowed: Boolean, cameraAllowed: Boolean): List<String> {
        val types = mutableListOf("phoneCall")
        if (inCall && micAllowed) types.add("microphone")
        if (inCall && video && cameraAllowed) types.add("camera")
        return types
    }

    // ---- the phone's dialer and contacts (plans/Comms-Messaging.md §9.1, §9.2; DialAccount, PhoneBook) ----

    /** How long a number handed over from the phone's dialer may be taken (Rust's `dial::LINK_MS`). */
    const val DIAL_LINK_MS = 120_000L

    /**
     * The check's state of the calling account: `off` (not registered), `registered` (the user may not have turned it on
     * yet: the app cannot read it) or `on` (a number came through it, so the user did).
     */
    fun dialAccountState(registered: Boolean, used: Boolean): String = when {
        !registered -> "off"
        used -> "on"
        else -> "registered"
    }

    /** A dialed number as a notification shows it: its digits, `+`, `*` and `#` only, at most 32. */
    fun dialShown(number: String): String = number.filter { it.isDigit() || it == '+' || it == '*' || it == '#' }.take(32)

    /**
     * A contact's number as the app takes it: the phone's E.164 form when it has one, else the number as stored with its
     * spaces, dashes, dots and brackets dropped (the server reads it by the deployment's rules), when it is 3 to 20 digits
     * with at most a leading `+`. Null: not a number to call or text.
     */
    fun contactNumber(normalized: String?, raw: String?): String? {
        val e164 = normalized?.trim()
        if (e164 != null && Regex("^\\+[0-9]{5,15}$").matches(e164)) return e164
        val plain = raw?.trim()?.filterNot { it == ' ' || it == '-' || it == '.' || it == '(' || it == ')' } ?: return null
        return plain.takeIf { Regex("^\\+?[0-9]{3,20}$").matches(it) }
    }

    /** Whether the phone's contacts are searched for [term]: two characters or more, at most 100. */
    fun contactSearchable(term: String): Boolean = term.trim().length in 2..100

    /**
     * Whether the contacts permission is asked now: not when it is granted, never twice by the app itself (a search is
     * its first use; the notification check asks again on the user's tap).
     */
    fun askContacts(granted: Boolean, askedBefore: Boolean): Boolean = !granted && !askedBefore
}
