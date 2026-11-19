// This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License.

package org.moqui.mobile

import android.Manifest
import android.app.Notification
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
        ChatNotifications.foreground = false
        ChatNotifications.roomShown(context, "", "")
        manager().cancelAll()
        context.getSharedPreferences("moqui-chat", Context.MODE_PRIVATE).edit().clear().commit()
    }

    private fun manager() = context.getSystemService(NotificationManager::class.java)!!

    private fun shownNow(tag: String) = manager().activeNotifications.firstOrNull { it.tag == tag }

    // the notification service posts and cancels on its own thread: wait for it to show (or, gone, to go)
    private fun shown(tag: String) = waitFor { shownNow(tag) != null }.let { shownNow(tag) }

    private fun gone(tag: String) = waitFor { shownNow(tag) == null }.let { shownNow(tag) }

    private fun payload(kind: String = "message", account: String = A) = JSONObject()
        .put("kind", kind).put("account", account).put("chatRoomId", "100").put("eventSeq", 5)
        .put("roomName", "Team").put("senderName", "Ann").put("snippet", "Lunch at noon?").put("count", 1)

    /** An action's intent as the notification makes it: the ids in its data, never its extras. */
    private fun actionIntent(what: String, account: String = A) = Intent(context, ChatActionReceiver::class.java)
        .setAction(what)
        .setData(ChatActionReceiver.actionUri(account, "100", 5L))

    private fun waitFor(what: () -> Boolean) {
        val until = System.currentTimeMillis() + 5_000
        while (!what() && System.currentTimeMillis() < until) Thread.sleep(50)
    }

    @Test
    fun aMessageShowsTheRoomsNotificationAndAClearRemovesIt() {
        ChatNotifications.show(context, payload())
        val room = shown(TAG_A)
        assertNotNull("the room's notification shows", room)
        assertEquals(ChatRules.MESSAGES, room!!.notification.channelId)
        val titles = room.notification.actions.map { it.title.toString() }
        assertEquals(listOf("Reply", "Mark read", "Mute 1 hour"), titles)
        // a real account id (with dots) has its room link and its conversation shortcut
        assertNotNull("a tap opens the room", room.notification.contentIntent)
        assertEquals("chat-$A-100", room.notification.shortcutId)
        ChatNotifications.show(context, JSONObject().put("kind", "clear").put("account", A).put("chatRoomId", "100"))
        assertNull("a clear removes it", gone(TAG_A))
    }

    /** Review finding: another account's room 100 (another server's) is another notification, cleared on its own. */
    @Test
    fun aRoomOfAnotherAccountIsAnotherNotification() {
        ChatNotifications.show(context, payload())
        ChatNotifications.show(context, payload(account = B))
        assertNotNull(shown(TAG_A))
        assertNotNull(shown(TAG_B))
        // the page read B's room (notify_clear): only B's goes
        ChatNotifications.clearRoom(context, B, "100")
        assertNull(gone(TAG_B))
        assertNotNull(shownNow(TAG_A))
    }

    @Test
    fun aMentionGoesToMentionsAndAnEncryptedRoomShowsNoText() {
        ChatNotifications.show(context, payload().put("mention", true))
        assertEquals(ChatRules.MENTIONS, shown(TAG_A)!!.notification.channelId)
        val encrypted = payload().put("chatRoomId", "101").put("encrypted", true)
        encrypted.remove("snippet")
        ChatNotifications.show(context, encrypted)
        val sealed = shown(ChatRules.tag(A, "101"))!!.notification
        val text = sealed.extras.getCharSequence("android.text").toString()
        assertTrue(text, text.contains("open the app"))
    }

    /**
     * SEC-CLI-115: a text's one-time code is secret on a secure lock screen, with no public version (shown only when the
     * user set the lock screen to hide sensitive content, so the code would show under the default); Copy code is there.
     */
    @Test
    fun aOneTimeCodeIsSecretOnTheLockScreen() {
        ChatNotifications.show(context, payload().put("snippet", "Your code is 123456").put("otp", JSONObject().put("code", "123456")))
        val coded = shown(TAG_A)!!.notification
        assertEquals(Notification.VISIBILITY_SECRET, coded.visibility)
        assertNull("no public version", coded.publicVersion)
        assertTrue(coded.actions.map { it.title.toString() }.contains("Copy code"))
    }

    @Test
    fun replyGoesThroughTheOutbox() {
        ChatNotifications.show(context, payload())
        val intent = actionIntent(ChatActionReceiver.REPLY)
        val input = RemoteInput.Builder(ChatActionReceiver.REPLY_TEXT).build()
        RemoteInput.addResultsToIntent(arrayOf(input), intent, Bundle().apply { putCharSequence(ChatActionReceiver.REPLY_TEXT, "Yes") })
        context.sendBroadcast(intent)
        waitFor { synchronized(replies) { replies.isNotEmpty() } }
        assertEquals(listOf("$A/100: Yes"), synchronized(replies) { replies.toList() })
        assertNull("the room's notification goes after the reply", gone(TAG_A))
        ChatNotifications.show(context, payload())
        context.sendBroadcast(actionIntent(ChatActionReceiver.READ))
        waitFor { synchronized(reads) { reads.isNotEmpty() } }
        assertEquals(listOf(5L), synchronized(reads) { reads.toList() })
    }

    /** Review finding: the ids are the intent's data; extras (which a mutable intent's fill-in could add) are not read. */
    @Test
    fun anActionTakesItsRoomFromItsDataNotItsExtras() {
        val forged = Intent(context, ChatActionReceiver::class.java).setAction(ChatActionReceiver.READ)
            .putExtra("account", A).putExtra("chatRoomId", "100").putExtra("eventSeq", 5L)
        context.sendBroadcast(forged)
        Thread.sleep(500)
        assertTrue(synchronized(reads) { reads.isEmpty() })
    }

    @Test
    fun aReplyNotQueuedSaysSoAndKeepsItsText() {
        ChatActionReceiver.outbox = object : ChatActionReceiver.Outbox {
            override fun reply(context: Context, account: String, room: String, text: String): String? = "not signed in"
            override fun read(context: Context, account: String, room: String, eventSeq: Long): String? = "not signed in"
        }
        ChatNotifications.show(context, payload())
        val intent = actionIntent(ChatActionReceiver.REPLY)
        RemoteInput.addResultsToIntent(arrayOf(RemoteInput.Builder(ChatActionReceiver.REPLY_TEXT).build()), intent,
            Bundle().apply { putCharSequence(ChatActionReceiver.REPLY_TEXT, "Kept") })
        // the notification service sheds a package's updates over about 5 a second (the tests before posted many)
        Thread.sleep(1_500)
        context.sendBroadcast(intent)
        waitFor { shownNow(TAG_A)?.notification?.extras?.getCharSequence("android.text")?.toString() == "Kept" }
        val failed = shownNow(TAG_A)!!.notification
        assertEquals("Kept", failed.extras.getCharSequence("android.text").toString())
        assertTrue(failed.extras.getCharSequence("android.title").toString().contains("not sent"))
    }

    @Test
    fun aMutedRoomShowsNothingForAnHour() {
        ChatNotifications.mute(context, A, "100")
        ChatNotifications.show(context, payload())
        Thread.sleep(300)
        assertNull(shownNow(TAG_A))
    }

    /** The room the app shows in front shows no notification; another room does. */
    @Test
    fun theRoomInViewShowsNoNotification() {
        ChatNotifications.foreground = true
        ChatNotifications.roomShown(context, A, "100")
        ChatNotifications.show(context, payload())
        ChatNotifications.show(context, payload().put("chatRoomId", "101"))
        assertNotNull(shown(ChatRules.tag(A, "101")))
        assertNull(shownNow(TAG_A))
    }

    /**
     * Review findings: a sealed push that opens with no account's keys here is dropped, not shown as a bare "New
     * message"; a sealed ring that does not open shows a missed call rather than nothing.
     */
    @Test
    fun aPushThatDoesNotOpenIsDroppedAndARingOfNoAccountHereToo() {
        Push.onMessage(context, mapOf("kind" to "chat", "pushId" to "100050", "enc" to "bm90IGEgcHVzaA"))
        Push.onMessage(context, mapOf("kind" to "ring", "pushId" to "100050", "enc" to "bm90IGEgcHVzaA"))
        Thread.sleep(1_000)
        // no account here has that push id: no missed call either (one is shown only for a ring of an account here)
        assertNull("a ring of no account here: nothing", shownNow("ring-not-opened"))
        assertNull("a push that does not open: nothing", shownNow("chat-bare"))
    }

    /** Review finding: clearing one of two rooms of an account leaves the other, and its summary, showing. */
    @Test
    fun clearingOneOfTwoRoomsLeavesTheOther() {
        ChatNotifications.show(context, payload())
        Thread.sleep(400)
        ChatNotifications.show(context, payload().put("chatRoomId", "101"))
        val other = ChatRules.tag(A, "101")
        assertNotNull(shown(TAG_A))
        assertNotNull(shown(other))
        waitFor { shownNow(ChatRules.summaryTag(A)) != null }
        ChatNotifications.clear(context, A, "100")
        assertNull(gone(TAG_A))
        Thread.sleep(500)
        assertNotNull("the other room stays", shownNow(other))
        // Mark all read on the phone: every room of the account goes
        ChatNotifications.clearRoom(context, A, "*")
        assertNull(gone(other))
        assertNull(gone(ChatRules.summaryTag(A)))
    }

    /**
     * Review finding (HIGH): the connector's receiver opens with its own key manager, which has no keys of the account,
     * so a message comes still sealed; the app opens it with the account's keys and shows it. One the connector says it
     * opened (sealed to the connector's own keys) or that does not open is dropped.
     */
    @Test
    fun aUnifiedPushMessageIsOpenedWithTheAccountsKeys() {
        val sealed = "sealed".toByteArray()
        val plain = payload().apply { remove("account") }.toString().toByteArray()
        val opener: (ByteArray) -> ByteArray? = { bytes -> if (bytes.contentEquals(sealed)) plain else null }
        UnifiedPushService.handle(context, org.unifiedpush.android.connector.data.PushMessage(sealed, false), A, opener)
        assertNotNull("a sealed message opened with the account's keys shows", shown(TAG_A))
        manager().cancelAll()
        Thread.sleep(1_500)
        UnifiedPushService.handle(context, org.unifiedpush.android.connector.data.PushMessage(plain, true), A, opener)
        UnifiedPushService.handle(context, org.unifiedpush.android.connector.data.PushMessage("other".toByteArray(), false), A, opener)
        Thread.sleep(800)
        assertNull("opened by the connector's own keys, or not opening: nothing", shownNow(TAG_A))
    }

    private companion object {
        /** Real account ids: a host with dots, a `-` and a hex digest. */
        const val A = "chat.example.com-0123456789ab"
        const val B = "chat.example.org-ba9876543210"
        val TAG_A = ChatRules.tag(A, "100")
        val TAG_B = ChatRules.tag(B, "100")
    }
}
