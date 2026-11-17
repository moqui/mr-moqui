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
 * each message encrypted to the app ([Push.onMessage]: `{kind: chat | clear | test | notification, pushId, enc}`),
 * Rust opens it with the app's push keys (`Native.openPush`, src/push.rs), and this shows it: one notification per
 * room ([ChatRules.tag], the server's), in the channel of its kind ([ChatRules.channel]), a `MessagingStyle` with the
 * sender, grouped under a summary per account, with a conversation shortcut; Reply (inline), Mark read and Mute 1 hour
 * ([ChatActionReceiver]); a tap opens the room at the message (`org.moqui.mobile:/room`, src/lib.rs). An encrypted
 * room's push has the room and the sender only, and its Reply opens the app (the page encrypts). A `clear` (the room
 * read on another device) removes the room's notification. Each push shown is acknowledged (`Native.ackPush`, System,
 * Push Health) and kept as the last one received ([PushCheck]).
 */
object ChatNotifications {
    const val TAG = "moqui-push"
    /** The notification id of every room notification: the tag tells them apart. */
    private const val ROOM_ID = 1
    private const val SUMMARY_ID = 2
    private val main = Handler(Looper.getMainLooper())

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

    /** A push that is not a call's: opened by Rust off the main thread, then shown. */
    fun onPush(context: Context, data: Map<String, String>) {
        val app = context.applicationContext
        val pushId = data["pushId"] ?: ""
        val enc = data["enc"]
        Native.work {
            if (!Native.ensure(app)) {
                Log.w(TAG, "The app's library is not ready: a push is not shown")
                return@work
            }
            val opened = if (enc != null) Native.openPush(Native.dataDir(app), pushId, enc) else null
            val payload = try {
                opened?.let { JSONObject(it) }
            } catch (err: Exception) {
                null
            }
            main.post {
                when {
                    payload != null -> show(app, payload)
                    // a device registered before its keys: a push with no text, which says only that something came
                    data["kind"] == "chat" -> showBare(app)
                    else -> Log.i(TAG, "A push of kind ${data["kind"]} that does not open is not shown")
                }
            }
        }
    }

    /** Show an opened push (its JSON, with `account`), on the main thread. */
    fun show(context: Context, payload: JSONObject) {
        ensureChannels(context)
        val account = payload.optString("account", "")
        val kind = payload.optString("kind", "")
        when (kind) {
            "clear" -> clear(context, account, payload.optString("chatRoomId", ""))
            // a test shows as a message does, so it checks the channel that matters
            "test" -> showOther(context, payload, context.getString(R.string.push_test_title), context.getString(R.string.push_test_body), ChatRules.MESSAGES)
            "notification" -> showOther(context, payload, payload.optString("title", context.getString(R.string.chat_new_message)), null, ChatRules.OTHER)
            "message" -> showRoom(context, account, payload)
            else -> Log.i(TAG, "A push of kind $kind is not shown")
        }
        if (kind != "clear") {
            PushCheck.received(context, kind, payload.optString("ackId", ""))
            val ackId = payload.optString("ackId", "")
            if (account.isNotEmpty() && ackId.isNotEmpty()) {
                val app = context.applicationContext
                Native.work { Native.ackPush(Native.dataDir(app), account, ackId) }
            }
        }
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

    private fun showOther(context: Context, payload: JSONObject, title: String, body: String?, channel: String) {
        if (!allowed(context)) return
        val builder = NotificationCompat.Builder(context, channel)
            .setSmallIcon(R.mipmap.ic_launcher)
            .setContentTitle(title)
            .setContentIntent(openApp(context, null))
            .setAutoCancel(true)
        if (body != null) builder.setContentText(body)
        notify(context, payload.optString("tag", "moqui-other"), ROOM_ID, builder)
    }

    private fun showRoom(context: Context, account: String, payload: JSONObject) {
        val room = ChatRules.plainId(payload.optString("chatRoomId", "")) ?: return
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
        val link = ChatRules.roomLink(account, room, eventSeq)
        val shortcutId = "chat-$account-$room"
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
        notify(context, ChatRules.tag(room), ROOM_ID, builder)
        summary(context, account)
    }

    /** The summary of an account's room notifications, which groups them. */
    private fun summary(context: Context, account: String) {
        val rooms = roomTags(context, account)
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

    /** How many room notifications of `account` show. */
    private fun roomTags(context: Context, account: String): Int {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.M) return 0
        val manager = context.getSystemService(NotificationManager::class.java) ?: return 0
        return manager.activeNotifications.count {
            it.id == ROOM_ID && it.tag?.startsWith("chat-") == true && it.notification.group == ChatRules.group(account)
        }
    }

    /** A room read here or on another device: its notification goes, and the summary with the last one. */
    fun clear(context: Context, account: String, room: String) {
        val plain = ChatRules.plainId(room) ?: return
        manager(context).cancel(ChatRules.tag(plain), ROOM_ID)
        if (roomTags(context, account) == 0) manager(context).cancel(ChatRules.summaryTag(account), SUMMARY_ID)
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
            .setContentIntent(openApp(context, ChatRules.roomLink(account, plain, 0L)))
            .setAutoCancel(true)
            .setOnlyAlertOnce(true)
        notify(context, ChatRules.tag(plain), ROOM_ID, builder)
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
        if (link == null) return
        try {
            val shortcut = ShortcutInfoCompat.Builder(context, id)
                .setShortLabel(name.take(40))
                .setLongLived(true)
                .setIcon(IconCompat.createWithResource(context, R.mipmap.ic_launcher))
                .setIntent(Intent(Intent.ACTION_VIEW, Uri.parse(link)).setPackage(context.packageName))
                .build()
            ShortcutManagerCompat.pushDynamicShortcut(context, shortcut)
        } catch (err: Exception) {
            Log.w(TAG, "No conversation shortcut for the room: ${err.message}")
        }
    }

    /** What a tap opens: the room at the message (a link the app takes, src/lib.rs), or the app. */
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

    /** A room's action, to [ChatActionReceiver]; a Reply's intent is mutable, for the text Android fills in. */
    private fun action(context: Context, what: String, room: RoomAction, mutable: Boolean): PendingIntent {
        val intent = Intent(context, ChatActionReceiver::class.java)
            .setAction(what)
            .putExtra(ChatActionReceiver.ACCOUNT, room.account)
            .putExtra(ChatActionReceiver.ROOM, room.room)
            .putExtra(ChatActionReceiver.EVENT_SEQ, room.eventSeq)
        val flags = PendingIntent.FLAG_UPDATE_CURRENT or when {
            !mutable -> PendingIntent.FLAG_IMMUTABLE
            Build.VERSION.SDK_INT >= Build.VERSION_CODES.S -> PendingIntent.FLAG_MUTABLE
            else -> 0
        }
        return PendingIntent.getBroadcast(context, "$what:${room.account}:${room.room}".hashCode(), intent, flags)
    }

    private data class RoomAction(val account: String, val room: String, val eventSeq: Long)
}
