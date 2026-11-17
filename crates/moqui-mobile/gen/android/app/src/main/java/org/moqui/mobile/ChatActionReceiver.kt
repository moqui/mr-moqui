// This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License.

package org.moqui.mobile

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.util.Log
import androidx.core.app.RemoteInput

/**
 * A chat notification's actions (plans/Comms-Messaging.md §7.4, Actions; [ChatNotifications]): Reply (its inline text)
 * and Mark read go to the account's outbox in Rust (`chat-send`, `chat-read`: `Native.chatReply`, `Native.chatRead`),
 * which sends them with no WebView, offline too ([FlushWorker]); Mute 1 hour mutes the room on this phone. The room's
 * notification goes after each.
 */
class ChatActionReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        val app = context.applicationContext
        val account = intent.getStringExtra(ACCOUNT) ?: return
        val room = ChatRules.plainId(intent.getStringExtra(ROOM)) ?: return
        val eventSeq = intent.getLongExtra(EVENT_SEQ, 0L)
        when (intent.action) {
            REPLY -> {
                val text = RemoteInput.getResultsFromIntent(intent)?.getCharSequence(REPLY_TEXT)?.toString()?.trim()
                if (text.isNullOrEmpty()) return
                val pending = goAsync()
                Native.work {
                    var sent = false
                    try {
                        val why = outbox.reply(app, account, room, text)
                        if (why != null) Log.w(ChatNotifications.TAG, "The reply was not queued: $why")
                        else { sent = true; FlushWorker.background(app) }
                    } finally {
                        // queued: the room's notification goes; not: it says so, with the text kept to send from the app
                        if (sent) ChatNotifications.clear(app, account, room) else ChatNotifications.replyFailed(app, account, room, text)
                        pending.finish()
                    }
                }
            }
            READ -> {
                val pending = goAsync()
                Native.work {
                    try {
                        val why = outbox.read(app, account, room, eventSeq)
                        if (why != null) Log.w(ChatNotifications.TAG, "The read was not queued: $why")
                        else FlushWorker.background(app)
                    } finally {
                        ChatNotifications.clear(app, account, room)
                        pending.finish()
                    }
                }
            }
            MUTE -> ChatNotifications.mute(app, account, room)
        }
    }

    /** Where a Reply and a Mark read go: the account's outbox in Rust; a test's stand-in. */
    interface Outbox {
        /** Queue `text` to the room; null, or why not. */
        fun reply(context: Context, account: String, room: String, text: String): String?

        /** Queue the room read up to `eventSeq`; null, or why not. */
        fun read(context: Context, account: String, room: String, eventSeq: Long): String?
    }

    /** The outbox of the app's Rust client (src/native.rs chat_reply, chat_read). */
    object RustOutbox : Outbox {
        override fun reply(context: Context, account: String, room: String, text: String): String? {
            if (!Native.ensure(context)) return "the app's library is not ready"
            return Native.chatReply(Native.dataDir(context), account, room, text)
        }

        override fun read(context: Context, account: String, room: String, eventSeq: Long): String? {
            if (!Native.ensure(context)) return "the app's library is not ready"
            return Native.chatRead(Native.dataDir(context), account, room, eventSeq)
        }
    }

    companion object {
        const val REPLY = "org.moqui.mobile.chat.REPLY"
        const val READ = "org.moqui.mobile.chat.READ"
        const val MUTE = "org.moqui.mobile.chat.MUTE"
        const val ACCOUNT = "account"
        const val ROOM = "chatRoomId"
        const val EVENT_SEQ = "eventSeq"
        const val REPLY_TEXT = "replyText"

        /** The outbox the actions go to (the tests put their own). */
        @Volatile var outbox: Outbox = RustOutbox
    }
}
