// This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License.

package org.moqui.mobile

import android.Manifest
import android.app.NotificationManager
import android.content.Context
import android.content.Intent
import android.os.Build
import android.os.Bundle
import androidx.core.app.RemoteInput
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import org.json.JSONObject
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Test
import org.junit.runner.RunWith

/**
 * The chat notifications on a device (plans/Comms-Messaging.md §7.6, Android instrumented tests; run on request on
 * the emulator, as docs/ui/Mobile-Client.md#tests says): an opened push (the payload Rust gives, as a fake FCM message
 * would after `Native.openPush`) shows the room's notification with its actions, Reply hands its text to the outbox,
 * and a `clear` removes it.
 */
@RunWith(AndroidJUnit4::class)
class ChatNotificationsTest {
    private lateinit var context: Context
    private val replies = mutableListOf<String>()
    private val reads = mutableListOf<Long>()

    @Before
    fun setUp() {
        context = InstrumentationRegistry.getInstrumentation().targetContext
        if (Build.VERSION.SDK_INT >= 33) {
            InstrumentationRegistry.getInstrumentation().uiAutomation
                .grantRuntimePermission(context.packageName, Manifest.permission.POST_NOTIFICATIONS)
        }
        context.getSharedPreferences("moqui-chat", Context.MODE_PRIVATE).edit().clear().commit()
        ChatActionReceiver.outbox = object : ChatActionReceiver.Outbox {
            override fun reply(context: Context, account: String, room: String, text: String): String? {
                synchronized(replies) { replies.add("$account/$room: $text") }
                return null
            }

            override fun read(context: Context, account: String, room: String, eventSeq: Long): String? {
                synchronized(reads) { reads.add(eventSeq) }
                return null
            }
        }
    }

    @After
    fun tearDown() {
        ChatActionReceiver.outbox = ChatActionReceiver.RustOutbox
        manager().cancelAll()
        context.getSharedPreferences("moqui-chat", Context.MODE_PRIVATE).edit().clear().commit()
    }

    private fun manager() = context.getSystemService(NotificationManager::class.java)!!

    private fun shownNow(tag: String) = manager().activeNotifications.firstOrNull { it.tag == tag }

    // the notification service posts and cancels on its own thread: wait for it to show (or, gone, to go)
    private fun shown(tag: String) = waitFor { shownNow(tag) != null }.let { shownNow(tag) }

    private fun gone(tag: String) = waitFor { shownNow(tag) == null }.let { shownNow(tag) }

    private fun payload(kind: String = "message") = JSONObject()
        .put("kind", kind).put("account", "a1").put("chatRoomId", "100").put("eventSeq", 5)
        .put("roomName", "Team").put("senderName", "Ann").put("snippet", "Lunch at noon?").put("count", 1)

    private fun waitFor(what: () -> Boolean) {
        val until = System.currentTimeMillis() + 5_000
        while (!what() && System.currentTimeMillis() < until) Thread.sleep(50)
    }

    @Test
    fun aMessageShowsTheRoomsNotificationAndAClearRemovesIt() {
        ChatNotifications.show(context, payload())
        val room = shown("chat-100")
        assertNotNull("the room's notification shows", room)
        assertEquals(ChatRules.MESSAGES, room!!.notification.channelId)
        val titles = room.notification.actions.map { it.title.toString() }
        assertEquals(listOf("Reply", "Mark read", "Mute 1 hour"), titles)
        ChatNotifications.show(context, JSONObject().put("kind", "clear").put("account", "a1").put("chatRoomId", "100"))
        assertNull("a clear removes it", gone("chat-100"))
    }

    @Test
    fun aMentionGoesToMentionsAndAnEncryptedRoomShowsNoText() {
        ChatNotifications.show(context, payload().put("mention", true))
        assertEquals(ChatRules.MENTIONS, shown("chat-100")!!.notification.channelId)
        val encrypted = payload().put("chatRoomId", "101").put("encrypted", true)
        encrypted.remove("snippet")
        ChatNotifications.show(context, encrypted)
        val sealed = shown("chat-101")!!.notification
        val text = sealed.extras.getCharSequence("android.text").toString()
        assertTrue(text, text.contains("open the app"))
    }

    @Test
    fun replyGoesThroughTheOutbox() {
        ChatNotifications.show(context, payload())
        val intent = Intent(context, ChatActionReceiver::class.java)
            .setAction(ChatActionReceiver.REPLY)
            .putExtra(ChatActionReceiver.ACCOUNT, "a1")
            .putExtra(ChatActionReceiver.ROOM, "100")
            .putExtra(ChatActionReceiver.EVENT_SEQ, 5L)
        val input = RemoteInput.Builder(ChatActionReceiver.REPLY_TEXT).build()
        RemoteInput.addResultsToIntent(arrayOf(input), intent, Bundle().apply { putCharSequence(ChatActionReceiver.REPLY_TEXT, "Yes") })
        context.sendBroadcast(intent)
        waitFor { synchronized(replies) { replies.isNotEmpty() } }
        assertEquals(listOf("a1/100: Yes"), synchronized(replies) { replies.toList() })
        assertNull("the room's notification goes after the reply", gone("chat-100"))
        ChatNotifications.show(context, payload())
        context.sendBroadcast(Intent(context, ChatActionReceiver::class.java).setAction(ChatActionReceiver.READ)
            .putExtra(ChatActionReceiver.ACCOUNT, "a1").putExtra(ChatActionReceiver.ROOM, "100").putExtra(ChatActionReceiver.EVENT_SEQ, 5L))
        waitFor { synchronized(reads) { reads.isNotEmpty() } }
        assertEquals(listOf(5L), synchronized(reads) { reads.toList() })
    }

    @Test
    fun aReplyNotQueuedSaysSoAndKeepsItsText() {
        ChatActionReceiver.outbox = object : ChatActionReceiver.Outbox {
            override fun reply(context: Context, account: String, room: String, text: String): String? = "not signed in"
            override fun read(context: Context, account: String, room: String, eventSeq: Long): String? = "not signed in"
        }
        ChatNotifications.show(context, payload())
        val intent = Intent(context, ChatActionReceiver::class.java).setAction(ChatActionReceiver.REPLY)
            .putExtra(ChatActionReceiver.ACCOUNT, "a1").putExtra(ChatActionReceiver.ROOM, "100").putExtra(ChatActionReceiver.EVENT_SEQ, 5L)
        RemoteInput.addResultsToIntent(arrayOf(RemoteInput.Builder(ChatActionReceiver.REPLY_TEXT).build()), intent,
            Bundle().apply { putCharSequence(ChatActionReceiver.REPLY_TEXT, "Kept") })
        // the notification service sheds a package's updates over about 5 a second (the tests before posted many)
        Thread.sleep(1_500)
        context.sendBroadcast(intent)
        waitFor { shownNow("chat-100")?.notification?.extras?.getCharSequence("android.text")?.toString() == "Kept" }
        val failed = shownNow("chat-100")!!.notification
        assertEquals("Kept", failed.extras.getCharSequence("android.text").toString())
        assertTrue(failed.extras.getCharSequence("android.title").toString().contains("not sent"))
    }

    @Test
    fun aMutedRoomShowsNothingForAnHour() {
        ChatNotifications.mute(context, "a1", "100")
        ChatNotifications.show(context, payload())
        Thread.sleep(300)
        assertNull(shownNow("chat-100"))
    }
}
