// This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License.

package org.moqui.mobile

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
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

    /** §8.2 During: earpiece for voice, speaker for video; a headset stays; the user's choice stays. */
    @Test
    fun aCallGoesToTheEarpieceForVoiceTheSpeakerForVideoAndAHeadsetStays() {
        val phone = listOf(CallRules.EARPIECE, CallRules.SPEAKER)
        assertEquals(CallRules.EARPIECE, CallRules.defaultRoute(false, phone, CallRules.SPEAKER, false))
        assertEquals(CallRules.SPEAKER, CallRules.defaultRoute(true, phone, CallRules.EARPIECE, false))
        assertNull(CallRules.defaultRoute(false, phone, CallRules.EARPIECE, false))
        assertNull("chosen by the user", CallRules.defaultRoute(true, phone, CallRules.EARPIECE, true))
        val headset = phone + CallRules.BLUETOOTH
        assertNull("a headset stays", CallRules.defaultRoute(true, headset, CallRules.BLUETOOTH, false))
        assertEquals("no earpiece (a tablet)", CallRules.SPEAKER, CallRules.defaultRoute(false, listOf(CallRules.SPEAKER), null, false))
        // a headset that comes takes the call; when it goes, the call's default comes back
        assertEquals(CallRules.BLUETOOTH, CallRules.afterRoutesChanged(false, phone, headset, CallRules.EARPIECE))
        assertEquals(CallRules.WIRED, CallRules.afterRoutesChanged(true, phone, phone + CallRules.WIRED, CallRules.SPEAKER))
        assertEquals(CallRules.EARPIECE, CallRules.afterRoutesChanged(false, headset, phone, CallRules.BLUETOOTH))
        assertNull(CallRules.afterRoutesChanged(false, phone, phone, CallRules.EARPIECE))
    }

    @Test
    fun theScreenGoesOffAtTheEarAndAVideoCallGoesToPictureInPicture() {
        assertTrue(CallRules.proximityWanted(true, false, CallRules.EARPIECE))
        assertFalse(CallRules.proximityWanted(true, false, CallRules.SPEAKER))
        assertFalse(CallRules.proximityWanted(true, true, CallRules.EARPIECE))
        assertFalse(CallRules.proximityWanted(false, false, CallRules.EARPIECE))
        assertTrue(CallRules.pipWanted(true, true, true))
        assertFalse(CallRules.pipWanted(true, false, true))
        assertFalse(CallRules.pipWanted(true, true, false))
    }

    /** §8.2 Leave the app: a call keeps its microphone and camera; a ring alone (from the background) has neither. */
    @Test
    fun theServiceHasTheMicrophoneAndCameraOfACallNotOfARing() {
        assertEquals(listOf("phoneCall"), CallRules.serviceTypes(false, true, true, true))
        assertEquals(listOf("phoneCall", "microphone"), CallRules.serviceTypes(true, false, true, true))
        assertEquals(listOf("phoneCall", "microphone", "camera"), CallRules.serviceTypes(true, true, true, true))
        assertEquals(listOf("phoneCall", "microphone"), CallRules.serviceTypes(true, true, true, false))
        assertEquals(listOf("phoneCall"), CallRules.serviceTypes(true, true, false, false))
    }
}
