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
    private val ACCOUNT = Regex("^[A-Za-z0-9][A-Za-z0-9.-]{0,299}$")

    /** The channel of a chat push: a missed call or a voicemail, a mention or a reply, an outside number's text, else a message. */
    fun channel(kind: String?, mention: Boolean, call: String?, external: Boolean): String = when {
        kind == "notification" -> OTHER
        // a test shows as a message does, so it checks the channel that matters
        kind == "test" -> MESSAGES
        call != null -> MISSED
        mention -> MENTIONS
        external -> TEXTS
        else -> MESSAGES
    }

    /**
     * The tag of a room's notification of an account (`chat-<account>/<room>`): another account's room of the same id
     * (another server's) is another notification, and a clear removes only that account's.
     */
    fun tag(account: String, room: String): String = "chat-$account/$room"

    /** The tag of the summary that groups an account's room notifications. */
    fun summaryTag(account: String): String = "chat-summary-$account"

    /** The group of an account's room notifications. */
    fun group(account: String): String = "chat-$account"

    /**
     * The tag of an account's other notification (a test, a reminder; `tag` the push's own), so the same tag of another
     * account is another notification and an account's sign-out finds its own ([ChatNotifications.forgetAccount]).
     */
    fun otherTag(account: String, tag: String): String = "other-$account/$tag"

    /** The id of the conversation shortcut of a room of an account. */
    fun shortcutId(account: String, room: String): String = "chat-$account-$room"

    /** A room id: letters, digits, `-` and `_`, as the server makes them. */
    fun plainId(text: String?): String? = text?.trim()?.takeIf { ID.matches(it) }

    /**
     * An account id as the app makes them (`moqui_client::account::account_id`): a host's letters, digits, `.` and `-`,
     * a `-` and a hex digest (`chat.example.com-0123456789ab`). The native code still looks it up among the accounts.
     */
    fun accountId(text: String?): String? = text?.trim()?.takeIf { ACCOUNT.matches(it) && !it.contains("..") }
    /** Whether a room muted until [untilMs] (0: not muted) is muted at [now]. */
    fun muted(untilMs: Long, now: Long): Boolean = untilMs > now

    /** The text of a room's notification: the snippet, or nothing when there is none (an encrypted room, previews off). */
    fun snippet(text: String?): String? = text?.trim()?.takeIf { it.isNotEmpty() }?.take(500)
}
