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
        assertEquals(ChatRules.OTHER, ChatRules.channel("test", false, null, false))
        assertEquals(ChatRules.OTHER, ChatRules.channel("notification", false, null, false))
    }

    @Test
    fun aRoomsTagIsTheServersAndItsLinkOnlyPlainIds() {
        assertEquals("chat-100", ChatRules.tag("100"))
        assertEquals("org.moqui.mobile:/room?account=a1&chatRoomId=100&eventSeq=7", ChatRules.roomLink("a1", "100", 7))
        assertEquals("org.moqui.mobile:/room?account=a1&chatRoomId=100", ChatRules.roomLink("a1", "100", 0))
        assertNull(ChatRules.roomLink("a1", "../x", 7))
        assertNull(ChatRules.roomLink("a b", "100", 7))
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
