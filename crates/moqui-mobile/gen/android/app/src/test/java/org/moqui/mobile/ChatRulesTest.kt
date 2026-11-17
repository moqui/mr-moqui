// This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License.

package org.moqui.mobile

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/** The chat notification's rules (plans/Comms-Messaging.md §7.4): channels, tags, links, mutes. */
class ChatRulesTest {
    @Test
    fun aPushGoesToTheChannelOfItsKind() {
        assertEquals(ChatRules.MESSAGES, ChatRules.channel("message", false, null, false))
        assertEquals(ChatRules.MENTIONS, ChatRules.channel("message", true, null, false))
        assertEquals(ChatRules.MISSED, ChatRules.channel("message", true, "missed", false))
        assertEquals(ChatRules.MISSED, ChatRules.channel("message", false, "voicemail", false))
        assertEquals(ChatRules.TEXTS, ChatRules.channel("message", false, null, true))
        assertEquals(ChatRules.MESSAGES, ChatRules.channel("test", false, null, false))
        assertEquals(ChatRules.OTHER, ChatRules.channel("notification", false, null, false))
    }

    /** Review findings: a real account id has dots; a room's tag is per account, so another server's room 100 is another. */
    @Test
    fun aRoomsTagIsPerAccountAndARealAccountIdIsOne() {
        val a = "chat.example.com-0123456789ab"
        val b = "chat.example.org-ba9876543210"
        assertEquals("chat-$a/100", ChatRules.tag(a, "100"))
        assertTrue(ChatRules.tag(a, "100") != ChatRules.tag(b, "100"))
        assertEquals("chat-summary-$a", ChatRules.summaryTag(a))
        assertEquals(a, ChatRules.accountId(" $a "))
        assertNull(ChatRules.accountId("a..b"))
        assertNull(ChatRules.accountId(".a"))
        assertNull(ChatRules.accountId("a/b"))
        assertEquals("100", ChatRules.plainId("100"))
        assertNull(ChatRules.plainId("../x"))
        assertNull("a room id has no dots", ChatRules.plainId(a))
    }

    @Test
    fun aMuteEndsAfterItsHour() {
        assertTrue(ChatRules.muted(1_000 + ChatRules.MUTE_MS, 1_000))
        assertFalse(ChatRules.muted(1_000, 1_000))
        assertFalse(ChatRules.muted(0, 1_000))
        assertNull(ChatRules.snippet("  "))
        assertEquals("Hi", ChatRules.snippet(" Hi "))
    }
}
