// This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License.

package org.moqui.mobile

import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.net.Uri
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.util.Log
import androidx.core.app.NotificationCompat
import androidx.core.app.NotificationManagerCompat
import androidx.core.app.Person
import androidx.core.app.RemoteInput
import androidx.core.content.pm.ShortcutInfoCompat
import androidx.core.content.pm.ShortcutManagerCompat
import androidx.core.graphics.drawable.IconCompat
import org.json.JSONObject

/**
 * The chat notifications, shown by the native code with no WebView (plans/Comms-Messaging.md §7.4): the server pushes
 * each message encrypted to the account's own push keys ([Push.onMessage]: `{kind: chat | clear | test | notification,
 * pushId, enc}` by FCM, or the whole payload by UnifiedPush, [UnifiedPushService]); Rust opens it, and only the account
 * whose keys open it is its (`Native.openPush`, src/push.rs); this shows it: one notification per room of an account
 * ([ChatRules.tag]), in the channel of its kind ([ChatRules.channel]), a `MessagingStyle` with the sender, grouped under a
 * summary per account, with a conversation shortcut; Reply (inline), Mark read and Mute 1 hour ([ChatActionReceiver]); a
 * tap opens the room at the message (a room link this app signed, `Native.roomLink`, src/lib.rs). An encrypted room's
 * push has the room and the sender only, and its Reply opens the app (the page encrypts). A `clear` (the room read on
 * another device) or a read in the app ([clearRoom]) removes the room's notification, and a room the app shows in front
 * ([roomShown]) shows none. Each push shown is acknowledged (`Native.ackPush`, System, Push Health) and noted for the
 * notification check ([PushCheck]).
 */
object ChatNotifications {
    const val TAG = "moqui-push"
    /** The notification id of every room notification: the tag tells them apart. */
    private const val ROOM_ID = 1
    private const val SUMMARY_ID = 2
    private val main = Handler(Looper.getMainLooper())

    /** Whether the app's activity is in front (MainActivity's onResume and onPause). */
    @Volatile var foreground = false

    /** The room the shell shows now, `<account>/<room>` (the page tells: `chat_room_shown`), or null. */
    @Volatile private var shownRoom: String? = null

    /** The app's channels, made once (idempotent): Messages, Mentions, Texts, Missed calls, Other. */
    fun ensureChannels(context: Context) {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.O) return
        val manager = context.getSystemService(NotificationManager::class.java) ?: return
        val made = listOf(
            Triple(ChatRules.MESSAGES, R.string.channel_messages, R.string.channel_messages_about) to NotificationManager.IMPORTANCE_DEFAULT,
            Triple(ChatRules.MENTIONS, R.string.channel_mentions, R.string.channel_mentions_about) to NotificationManager.IMPORTANCE_HIGH,
            Triple(ChatRules.TEXTS, R.string.channel_texts, R.string.channel_texts_about) to NotificationManager.IMPORTANCE_HIGH,
            Triple(ChatRules.MISSED, R.string.channel_missed, R.string.channel_missed_about) to NotificationManager.IMPORTANCE_DEFAULT,
            Triple(ChatRules.OTHER, R.string.channel_other, R.string.channel_other_about) to NotificationManager.IMPORTANCE_LOW,
        )
        for ((names, importance) in made) {
            if (manager.getNotificationChannel(names.first) != null) continue
            manager.createNotificationChannel(
                NotificationChannel(names.first, context.getString(names.second), importance).apply {
                    description = context.getString(names.third)
                }
            )
        }
    }

    /**
     * An FCM push that is not a call's: opened by Rust on the opener's thread, then shown. One with no `enc` (a device
     * registered before it had keys) says only that something came; one whose `enc` does not open with any account's
     * keys here is dropped (logged): it is no account's here, or not this app's.
     */
    fun onPush(context: Context, data: Map<String, String>) {
        val app = context.applicationContext
        val kind = data["kind"] ?: ""
        PushCheck.arrived(app, kind)
        val enc = data["enc"]
        if (enc == null) {
            if (kind == "chat") main.post { showBare(app) }
            else Log.i(TAG, "A push of kind $kind with nothing sealed is not shown")
            return
        }
        Native.open {
            if (!Native.ensure(app)) {
                Log.w(TAG, "The app's library is not ready: a push is not shown")
                return@open
            }
            val opened = Native.openPush(Native.dataDir(app), data["pushId"] ?: "", kind, enc)
            val payload = try {
                opened?.let { JSONObject(it) }
            } catch (err: Exception) {
                null
            }
            if (payload == null) {
                Log.w(TAG, "A push of kind $kind that does not open is dropped")
                return@open
            }
            main.post { show(app, payload) }
        }
    }

    /** Show an opened push (its JSON, with the `account` whose keys opened it), on the main thread. */
    fun show(context: Context, payload: JSONObject) {
        ensureChannels(context)
        val account = ChatRules.accountId(payload.optString("account", "")) ?: run {
            Log.w(TAG, "A push of no account is not shown")
            return
        }
        val kind = payload.optString("kind", "")
        when (kind) {
            "clear" -> clear(context, account, payload.optString("chatRoomId", ""))
            "test" -> showOther(context, account, payload, context.getString(R.string.push_test_title),
                context.getString(R.string.push_test_body), ChatRules.channel(kind, false, null, false))
            "notification" -> showOther(context, account, payload, payload.optString("title", context.getString(R.string.chat_new_message)),
                null, ChatRules.OTHER)
            "message" -> showRoom(context, account, payload)
            else -> Log.i(TAG, "A push of kind $kind is not shown")
        }
        if (kind != "clear") {
            val ackId = payload.optString("ackId", "")
            PushCheck.received(context, kind, ackId)
            if (ackId.isNotEmpty()) {
                val app = context.applicationContext
                Native.work { Native.ackPush(Native.dataDir(app), account, ackId) }
            }
        }
    }

    /**
     * A ring whose sealed call does not open: a missed-call notification that says a call came, rather than nothing
     * (the call itself is not shown, as its caller and id could not be read).
     */
    fun ringNotOpened(context: Context) {
        ensureChannels(context)
        if (!allowed(context)) return
        val builder = NotificationCompat.Builder(context, ChatRules.MISSED)
            .setSmallIcon(R.mipmap.ic_launcher)
            .setContentTitle(context.getString(R.string.chat_missed_call))
            .setContentText(context.getString(R.string.ring_not_opened))
            .setCategory(NotificationCompat.CATEGORY_MISSED_CALL)
            .setContentIntent(openApp(context, null))
            .setAutoCancel(true)
        notify(context, "ring-not-opened", ROOM_ID, builder)
    }

    private fun manager(context: Context) = NotificationManagerCompat.from(context)

    private fun allowed(context: Context): Boolean = manager(context).areNotificationsEnabled()

    private fun showBare(context: Context) {
        ensureChannels(context)
        if (!allowed(context)) return
        val builder = NotificationCompat.Builder(context, ChatRules.MESSAGES)
            .setSmallIcon(R.mipmap.ic_launcher)
            .setContentTitle(context.getString(R.string.chat_new_message))
            .setContentIntent(openApp(context, null))
            .setAutoCancel(true)
        notify(context, "chat-bare", ROOM_ID, builder)
    }

    /** A test or a reminder of `account`, tagged by the account ([ChatRules.otherTag]) so its sign-out finds it. */
    private fun showOther(context: Context, account: String, payload: JSONObject, title: String, body: String?, channel: String) {
        if (!allowed(context)) return
        val builder = NotificationCompat.Builder(context, channel)
            .setSmallIcon(R.mipmap.ic_launcher)
            .setContentTitle(title)
            .setContentIntent(openApp(context, null))
            .setAutoCancel(true)
        if (body != null) builder.setContentText(body)
        val tag = payload.optString("tag", "moqui-other").ifEmpty { "moqui-other" }
        notify(context, ChatRules.otherTag(account, tag), ROOM_ID, builder)
    }

    private fun showRoom(context: Context, account: String, payload: JSONObject) {
        val room = ChatRules.plainId(payload.optString("chatRoomId", "")) ?: return
        if (foreground && shownRoom == "$account/$room") {
            Log.i(TAG, "The room $room shows in the app: its push is not shown")
            return
        }
        val prefs = context.getSharedPreferences("moqui-chat", Context.MODE_PRIVATE)
        if (ChatRules.muted(prefs.getLong("mute.$account.$room", 0L), System.currentTimeMillis())) {
            Log.i(TAG, "The room $room is muted on this phone: its push is not shown")
            return
        }
        if (!allowed(context)) return
        val encrypted = payload.optBoolean("encrypted", false)
        val call = payload.optString("call", "").takeIf { it.isNotEmpty() && it != "null" }
        val mention = payload.optBoolean("mention", false)
        val channel = ChatRules.channel("message", mention, call, payload.optBoolean("external", false))
        val roomName = payload.optString("roomName", "").takeIf { it.isNotEmpty() && it != "null" }
        // no sender (a push of an encrypted room may name none): the room, else the app
        val sender = payload.optString("senderName", "").takeIf { it.isNotEmpty() && it != "null" }
            ?: roomName ?: context.applicationInfo.loadLabel(context.packageManager).toString()
        val direct = payload.optBoolean("direct", false)
        val count = payload.optInt("count", 1)
        val eventSeq = payload.optLong("eventSeq", 0L)
        val text = when (call) {
            "missed" -> context.getString(R.string.chat_missed_call)
            "voicemail" -> context.getString(R.string.chat_voicemail)
            else -> ChatRules.snippet(payload.optString("snippet", "").takeIf { it != "null" })
                ?: context.getString(if (encrypted) R.string.chat_open_to_read else R.string.chat_new_message)
        }
        val shown = if (count > 1) "$text (${context.getString(R.string.chat_new_messages, count)})" else text
        val me = Person.Builder().setName(context.getString(R.string.chat_you)).setKey("me").build()
        val from = Person.Builder().setName(sender).setKey("$account:$sender").build()
        val style = NotificationCompat.MessagingStyle(me)
            .setConversationTitle(if (direct) null else roomName)
            .setGroupConversation(!direct)
            .addMessage(shown, payload.optLong("sentDate", System.currentTimeMillis()), from)
        val link = roomLink(context, account, room, eventSeq)
        val shortcutId = ChatRules.shortcutId(account, room)
        pushShortcut(context, shortcutId, roomName ?: sender, link)
        val builder = NotificationCompat.Builder(context, channel)
            .setSmallIcon(R.mipmap.ic_launcher)
            .setStyle(style)
            .setContentTitle(roomName ?: sender)
            .setContentText(shown)
            .setNumber(count)
            .setCategory(if (call != null) NotificationCompat.CATEGORY_MISSED_CALL else NotificationCompat.CATEGORY_MESSAGE)
            .setShortcutId(shortcutId)
            .setGroup(ChatRules.group(account))
            .setContentIntent(openApp(context, link))
            .setAutoCancel(true)
            .setOnlyAlertOnce(false)
        val room0 = RoomAction(account, room, eventSeq)
        if (encrypted) {
            // the page encrypts: Reply opens the room
            builder.addAction(0, context.getString(R.string.chat_reply), openApp(context, link))
        } else {
            val input = RemoteInput.Builder(ChatActionReceiver.REPLY_TEXT).setLabel(context.getString(R.string.chat_reply_hint)).build()
            val reply = NotificationCompat.Action.Builder(0, context.getString(R.string.chat_reply), action(context, ChatActionReceiver.REPLY, room0, true))
                .addRemoteInput(input)
                .setAllowGeneratedReplies(true)
                .setSemanticAction(NotificationCompat.Action.SEMANTIC_ACTION_REPLY)
                .build()
            builder.addAction(reply)
        }
        builder.addAction(
            NotificationCompat.Action.Builder(0, context.getString(R.string.chat_mark_read), action(context, ChatActionReceiver.READ, room0, false))
                .setSemanticAction(NotificationCompat.Action.SEMANTIC_ACTION_MARK_AS_READ)
                .setShowsUserInterface(false)
                .build()
        )
        builder.addAction(0, context.getString(R.string.chat_mute_hour), action(context, ChatActionReceiver.MUTE, room0, false))
        // a text's one-time code (plans/Comms-Messaging.md §18.3 phase 21): Copy code, and secret on a secure lock screen
        // (SEC-CLI-115), which shows nothing of it, so Copy code needs the phone unlocked. Not VISIBILITY_PRIVATE with a
        // public version: Android shows that version only when the user set the lock screen to hide sensitive content,
        // so under its default, all content, the code would show.
        val code = payload.optJSONObject("otp")?.optString("code", "")?.takeIf { ChatActionReceiver.isCode(it) }
        if (code != null) {
            builder.addAction(0, context.getString(R.string.chat_copy_code), copyAction(context, room0, code))
            builder.setVisibility(NotificationCompat.VISIBILITY_SECRET)
        }
        notify(context, ChatRules.tag(account, room), ROOM_ID, builder)
        summary(context, account)
    }

    /** The summary of an account's room notifications, which groups them. */
    private fun summary(context: Context, account: String) {
        val rooms = roomTags(context, account).size
        if (rooms < 2) return
        val builder = NotificationCompat.Builder(context, ChatRules.MESSAGES)
            .setSmallIcon(R.mipmap.ic_launcher)
            .setContentTitle(context.getString(R.string.chat_summary, rooms))
            .setGroup(ChatRules.group(account))
            .setGroupSummary(true)
            .setContentIntent(openApp(context, null))
            .setAutoCancel(true)
            .setSilent(true)
        notify(context, ChatRules.summaryTag(account), SUMMARY_ID, builder)
    }

    /** The room notifications of `account` that show, but for the one of `except` (a tag). */
    private fun roomTags(context: Context, account: String, except: String? = null): List<String> =
        shownTags(context, ChatRules.tag(account, ""), except)

    /** The tags of the notifications that show whose tag starts with `prefix`, but for `except`. */
    private fun shownTags(context: Context, prefix: String, except: String? = null): List<String> {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.M) return emptyList()
        val manager = context.getSystemService(NotificationManager::class.java) ?: return emptyList()
        return manager.activeNotifications
            .filter { it.id == ROOM_ID && it.tag?.startsWith(prefix) == true && it.tag != except }
            .mapNotNull { it.tag }
    }

    /**
     * A room of `account` read here or on another device: its notification goes, and the summary only when no other
     * room's shows (cancelling a group's summary cancels its children, and the count is read before the cancel lands).
     */
    fun clear(context: Context, account: String, room: String) {
        val plain = ChatRules.plainId(room) ?: return
        val tag = ChatRules.tag(account, plain)
        val others = roomTags(context, account, tag)
        manager(context).cancel(tag, ROOM_ID)
        if (others.isEmpty()) manager(context).cancel(ChatRules.summaryTag(account), SUMMARY_ID)
    }

    /** Every room's notification of `account` goes (the page's Mark all read), and its summary. */
    fun clearAll(context: Context, account: String) {
        for (tag in roomTags(context, account)) manager(context).cancel(tag, ROOM_ID)
        manager(context).cancel(ChatRules.summaryTag(account), SUMMARY_ID)
    }

    /**
     * `account` is signed out here (`MobileHost::account_signed_out`, src/host.rs: every sign-out, one its server did not
     * hear included): its room notifications and their summary go ([clearAll]), and its other ones (a test, a reminder);
     * its conversation shortcuts ([ChatRules.shortcutId], long-lived, so kept by Android past the dynamic list) are
     * removed; and a room of it the shell showed is shown no more. Nothing of its chats (a sender, a snippet, a room's
     * name) stays for the next person to use the phone (SEC-CLI-100).
     */
    @JvmStatic
    fun forgetAccount(context: Context, account: String) {
        val plainAccount = ChatRules.accountId(account) ?: return
        val app = context.applicationContext
        clearAll(app, plainAccount)
        for (tag in shownTags(app, ChatRules.otherTag(plainAccount, ""))) manager(app).cancel(tag, ROOM_ID)
        if (shownRoom?.startsWith("$plainAccount/") == true) shownRoom = null
        try {
            val prefix = ChatRules.shortcutId(plainAccount, "")
            val ids = ShortcutManagerCompat.getShortcuts(
                app, ShortcutManagerCompat.FLAG_MATCH_DYNAMIC or ShortcutManagerCompat.FLAG_MATCH_CACHED
            ).map { it.id }.filter { it.startsWith(prefix) }
            // another account whose id starts with this one's and a `-` loses a shortcut too, made again by its next push
            if (ids.isNotEmpty()) ShortcutManagerCompat.removeLongLivedShortcuts(app, ids)
        } catch (err: Exception) {
            Log.w(TAG, "The conversation shortcuts of a signed-out account were not removed: ${err.message}")
        }
    }

    /** The page read a room of `account` (`notify_clear`, src/commands.rs; `*`: every room): its notification goes. */
    @JvmStatic
    fun clearRoom(context: Context, account: String, room: String) {
        val plainAccount = ChatRules.accountId(account) ?: return
        if (room == "*") clearAll(context.applicationContext, plainAccount)
        else clear(context.applicationContext, plainAccount, room)
    }

    /**
     * The room of `account` the shell shows now (empty: none; `chat_room_shown`, src/commands.rs): its pushes show no
     * notification while the app is in front, and the one it had goes.
     */
    @JvmStatic
    fun roomShown(context: Context, account: String, room: String) {
        val plainAccount = ChatRules.accountId(account)
        val plainRoom = ChatRules.plainId(room)
        if (plainAccount == null || plainRoom == null) {
            shownRoom = null
            return
        }
        shownRoom = "$plainAccount/$plainRoom"
        clear(context.applicationContext, plainAccount, plainRoom)
    }

    /** Mute the room on this phone for an hour ([ChatRules.MUTE_MS]). */
    fun mute(context: Context, account: String, room: String) {
        context.getSharedPreferences("moqui-chat", Context.MODE_PRIVATE).edit()
            .putLong("mute.$account.$room", System.currentTimeMillis() + ChatRules.MUTE_MS)
            .apply()
        clear(context, account, room)
    }

    /** A reply that could not be queued: the room's notification says so and keeps its text, and a tap opens the room. */
    fun replyFailed(context: Context, account: String, room: String, text: String) {
        val plain = ChatRules.plainId(room) ?: return
        if (!allowed(context)) return
        val builder = NotificationCompat.Builder(context, ChatRules.MESSAGES)
            .setSmallIcon(R.mipmap.ic_launcher)
            .setContentTitle(context.getString(R.string.chat_reply_failed))
            .setContentText(text)
            .setGroup(ChatRules.group(account))
            .setContentIntent(openApp(context, roomLink(context, account, plain, 0L)))
            .setAutoCancel(true)
            .setOnlyAlertOnce(true)
        notify(context, ChatRules.tag(account, plain), ROOM_ID, builder)
    }

    /** A room's tap link, signed by Rust so no other app can send one (`Native.roomLink`); null without the library. */
    private fun roomLink(context: Context, account: String, room: String, eventSeq: Long): String? {
        if (!Native.ensure(context)) return null
        return Native.roomLink(Native.dataDir(context), account, room, eventSeq)
    }

    private fun notify(context: Context, tag: String, id: Int, builder: NotificationCompat.Builder) {
        try {
            manager(context).notify(tag, id, builder.build())
        } catch (err: SecurityException) {
            Log.w(TAG, "Notifications are not allowed: ${err.message}")
        }
    }

    /** The conversation shortcut of a room, so Android shows its notification as a conversation. */
    private fun pushShortcut(context: Context, id: String, name: String, link: String?) {
        val intent = if (link != null) {
            Intent(Intent.ACTION_VIEW, Uri.parse(link)).setPackage(context.packageName)
        } else {
            context.packageManager.getLaunchIntentForPackage(context.packageName) ?: return
        }
        try {
            val shortcut = ShortcutInfoCompat.Builder(context, id)
                .setShortLabel(name.take(40))
                .setLongLived(true)
                .setIcon(IconCompat.createWithResource(context, R.mipmap.ic_launcher))
                .setIntent(intent)
                .build()
            ShortcutManagerCompat.pushDynamicShortcut(context, shortcut)
        } catch (err: Exception) {
            Log.w(TAG, "No conversation shortcut for the room: ${err.message}")
        }
    }

    /** What a tap opens: the room at the message (a link the app signed and takes, src/lib.rs), or the app. */
    private fun openApp(context: Context, link: String?): PendingIntent {
        val intent = if (link != null) {
            Intent(Intent.ACTION_VIEW, Uri.parse(link)).setPackage(context.packageName)
        } else {
            context.packageManager.getLaunchIntentForPackage(context.packageName) ?: Intent()
        }
        intent.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_SINGLE_TOP)
        return PendingIntent.getActivity(context, (link ?: "app").hashCode(), intent,
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE)
    }

    /**
     * A room's action, to [ChatActionReceiver]. The account, room and message are in the intent's data
     * ([ChatActionReceiver.actionUri]), which a mutable PendingIntent's fill-in cannot change (no `FILL_IN_DATA`); a
     * Reply's intent is mutable only for the text Android fills in.
     */
    private fun action(context: Context, what: String, room: RoomAction, mutable: Boolean): PendingIntent {
        val intent = Intent(context, ChatActionReceiver::class.java)
            .setAction(what)
            .setData(ChatActionReceiver.actionUri(room.account, room.room, room.eventSeq))
        val flags = PendingIntent.FLAG_UPDATE_CURRENT or when {
            !mutable -> PendingIntent.FLAG_IMMUTABLE
            Build.VERSION.SDK_INT >= Build.VERSION_CODES.S -> PendingIntent.FLAG_MUTABLE
            else -> 0
        }
        return PendingIntent.getBroadcast(context, 0, intent, flags)
    }

    /** Copy code: the code in the action's data (never its extras), to the receiver that puts it on the clipboard. */
    private fun copyAction(context: Context, room: RoomAction, code: String): PendingIntent {
        val intent = Intent(context, ChatActionReceiver::class.java)
            .setAction(ChatActionReceiver.COPY)
            .setData(ChatActionReceiver.actionUri(room.account, room.room, room.eventSeq).buildUpon()
                .appendQueryParameter(ChatActionReceiver.CODE, code).build())
        return PendingIntent.getBroadcast(context, 0, intent, PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE)
    }

    private data class RoomAction(val account: String, val room: String, val eventSeq: Long)
}
