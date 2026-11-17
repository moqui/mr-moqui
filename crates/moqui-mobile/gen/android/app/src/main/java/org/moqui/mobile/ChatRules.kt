// This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License.

package org.moqui.mobile

/**
 * The rules of a chat notification that need no Android (plans/Comms-Messaging.md §7.4), so the JVM tests check them
 * (app/src/test): the channel a push goes to, a room's tag, the room link a tap opens, and a mute's end.
 */
object ChatRules {
    const val MESSAGES = "chat-messages"
    const val MENTIONS = "chat-mentions"
    const val TEXTS = "chat-texts"
    const val MISSED = "calls-missed"
    const val OTHER = "other"

    /** Every channel of the app's own notifications, beside the calls' (`calls-ring`, `calls-ongoing`). */
    val CHANNELS = listOf(MESSAGES, MENTIONS, TEXTS, MISSED, OTHER)

    /** How long Mute 1 hour mutes a room on this phone. */
    const val MUTE_MS = 60 * 60 * 1000L

    private val ID = Regex("^[A-Za-z0-9_-]{1,64}$")

    /** The channel of a chat push: a missed call or a voicemail, a mention or a reply, an outside number's text, else a message. */
    fun channel(kind: String?, mention: Boolean, call: String?, external: Boolean): String = when {
        kind == "test" || kind == "notification" -> OTHER
        call != null -> MISSED
        mention -> MENTIONS
        external -> TEXTS
        else -> MESSAGES
    }

    /** The tag of a room's notification, the server's own (`chat-<room>`): a clear removes it by the same. */
    fun tag(room: String): String = "chat-$room"

    /** The tag of the summary that groups an account's room notifications. */
    fun summaryTag(account: String): String = "chat-summary-$account"

    /** The group of an account's room notifications. */
    fun group(account: String): String = "chat-$account"

    /** A room id, an account id: letters, digits, `-` and `_`, as the app's links take them. */
    fun plainId(text: String?): String? = text?.trim()?.takeIf { ID.matches(it) }

    /** The link a tap opens the app with: the room of the account at the message (src/calls.rs room_link_of). */
    fun roomLink(account: String, room: String, eventSeq: Long?): String? {
        val plainAccount = plainId(account) ?: return null
        val plainRoom = plainId(room) ?: return null
        val seq = if (eventSeq != null && eventSeq > 0) "&eventSeq=$eventSeq" else ""
        return "org.moqui.mobile:/room?account=$plainAccount&chatRoomId=$plainRoom$seq"
    }

    /** Whether a room muted until [untilMs] (0: not muted) is muted at [now]. */
    fun muted(untilMs: Long, now: Long): Boolean = untilMs > now

    /** The text of a room's notification: the snippet, or nothing when there is none (an encrypted room, previews off). */
    fun snippet(text: String?): String? = text?.trim()?.takeIf { it.isNotEmpty() }?.take(500)
}
