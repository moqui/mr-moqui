// This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License.

package org.moqui.mobile

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class CallRulesTest {
    @Test
    fun aRingEndsByTheServersTimeLeftNotThePhonesClock() {
        assertEquals(1_040_000L, CallRules.expiresAt(1_000_000, "40", "5"))
        assertEquals(7_000L, CallRules.expiresAt(1_000_000, null, "7000"))
        assertEquals(1_000_000 + CallRules.DEFAULT_RING_MS, CallRules.expiresAt(1_000_000, "x", null))
        assertEquals(1_000_000L, CallRules.expiresAt(1_000_000, "0", null))
    }

    @Test
    fun anAnsweredCallThePageNeverJoinsIsGivenUp() {
        assertFalse(CallRules.watchdogEnds(true, false, 0, CallRules.JOIN_WATCHDOG_MS - 1))
        assertTrue(CallRules.watchdogEnds(true, false, 0, CallRules.JOIN_WATCHDOG_MS))
        assertFalse(CallRules.watchdogEnds(true, true, 0, CallRules.JOIN_WATCHDOG_MS * 10))
        assertFalse(CallRules.watchdogEnds(false, false, 0, CallRules.JOIN_WATCHDOG_MS * 10))
    }

    @Test
    fun aRingsEndEndsWhatThePageDoesNotHold() {
        assertTrue(CallRules.ringEndedEnds(false, false, "answered"))
        // answered here, not joined: a time-out, a decline, a cancel end it; the user's other devices' "answered" not
        assertTrue(CallRules.ringEndedEnds(true, false, "timedOut"))
        assertTrue(CallRules.ringEndedEnds(true, false, "declined"))
        assertTrue(CallRules.ringEndedEnds(true, false, "ended"))
        assertFalse(CallRules.ringEndedEnds(true, false, "answered"))
        // joined: only the call's end
        assertFalse(CallRules.ringEndedEnds(true, true, "timedOut"))
        assertTrue(CallRules.ringEndedEnds(true, true, "ended"))
    }

    @Test
    fun onlyTheLinkMadeForTheAnsweredCallShowsOverTheLockScreen() {
        assertTrue(CallRules.ownsLink("7", "hN", "7", "hN", true))
        assertFalse(CallRules.ownsLink("7", "hN", "7", "hN", false))
        assertFalse(CallRules.ownsLink("7", null, "7", "hN", true))
        assertFalse(CallRules.ownsLink("7", "hX", "7", "hN", true))
        assertFalse(CallRules.ownsLink("8", "hN", "7", "hN", true))
        assertFalse(CallRules.ownsLink("7", null, "7", null, true))
    }
}
