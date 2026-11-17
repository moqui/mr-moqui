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
        // review finding: a headset that is there takes the call at its start, though Telecom named the earpiece first
        assertEquals(CallRules.BLUETOOTH, CallRules.defaultRoute(false, headset, CallRules.EARPIECE, false))
        assertEquals(CallRules.WIRED, CallRules.defaultRoute(true, phone + CallRules.WIRED, CallRules.SPEAKER, false))
        assertNull("the user's choice stays", CallRules.defaultRoute(false, headset, CallRules.EARPIECE, true))
        assertEquals("no earpiece (a tablet)", CallRules.SPEAKER, CallRules.defaultRoute(false, listOf(CallRules.SPEAKER), null, false))
        // a headset that comes takes the call; when it goes, the call's default comes back
        assertEquals(CallRules.BLUETOOTH, CallRules.afterRoutesChanged(false, phone, headset, CallRules.EARPIECE))
        assertEquals(CallRules.WIRED, CallRules.afterRoutesChanged(true, phone, phone + CallRules.WIRED, CallRules.SPEAKER))
        assertEquals(CallRules.EARPIECE, CallRules.afterRoutesChanged(false, headset, phone, CallRules.BLUETOOTH))
        assertNull(CallRules.afterRoutesChanged(false, phone, phone, CallRules.EARPIECE))
    }

    @Test
    fun theScreenGoesOffAtTheEarAndAVideoCallGoesToPictureInPicture() {
        assertTrue(CallRules.proximityWanted(true, false, CallRules.EARPIECE, true))
        assertFalse(CallRules.proximityWanted(true, false, CallRules.SPEAKER, true))
        assertFalse(CallRules.proximityWanted(true, true, CallRules.EARPIECE, true))
        assertFalse(CallRules.proximityWanted(false, false, CallRules.EARPIECE, true))
        assertFalse("not with the call screen gone or the app behind", CallRules.proximityWanted(true, false, CallRules.EARPIECE, false))
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

    /** §8.3 item 7: while one call is held, the call the app is in may not be held by Telecom (the page refuses). */
    @Test
    fun theCallInUseIsNotHoldableWhileAnotherIsHeld() {
        assertTrue(CallRules.holdable(anotherHeld = false, held = false))
        assertFalse(CallRules.holdable(anotherHeld = true, held = false))
        assertTrue("the held one itself", CallRules.holdable(anotherHeld = true, held = true))
        assertTrue("one on hold keeps the window", CallRules.pipStays(1))
        assertFalse(CallRules.pipStays(0))
    }

    /** An older call screen's offScreen after a newer one's screen leaves the screen shown. */
    @Test
    fun anOlderScreensOffScreenIsDropped() {
        assertTrue(CallRules.screenGoes(5L, 5L))
        assertFalse("an older mount", CallRules.screenGoes(4L, 5L))
        assertTrue("the host's own, no token", CallRules.screenGoes(null, 5L))
    }

    /** §8.3 item 5: Telecom's first mute report is not a change; the user's route is asked again after a Swap. */
    @Test
    fun theFirstMuteReportIsNotAChangeAndAChosenRouteComesBack() {
        assertFalse(CallRules.muteTold(first = true))
        assertTrue(CallRules.muteTold(first = false))
        assertEquals(CallRules.SPEAKER, CallRules.chosenAgain(CallRules.SPEAKER, listOf(CallRules.EARPIECE, CallRules.SPEAKER), CallRules.EARPIECE))
        assertNull("in use already", CallRules.chosenAgain(CallRules.SPEAKER, listOf(CallRules.SPEAKER), CallRules.SPEAKER))
        assertNull("gone", CallRules.chosenAgain(CallRules.BLUETOOTH, listOf(CallRules.EARPIECE), CallRules.EARPIECE))
        assertNull("none chosen", CallRules.chosenAgain(null, listOf(CallRules.EARPIECE), CallRules.EARPIECE))
    }

    /** §9.1: the calling account is off until registered, and known to be on once a number came through it. */
    @Test
    fun theCallingAccountIsOnOnceANumberCameThroughIt() {
        assertEquals("off", CallRules.dialAccountState(false, true))
        assertEquals("registered", CallRules.dialAccountState(true, false))
        assertEquals("on", CallRules.dialAccountState(true, true))
        assertEquals("+18015550100", CallRules.dialShown("+1 (801) 555-0100<b>"))
    }

    /** §9.2: a contact's number is its E.164 form, else the number as stored without its separators; a name is no number. */
    @Test
    fun aContactsNumberIsItsE164FormOrItsDigits() {
        assertEquals("+18015550100", CallRules.contactNumber("+18015550100", "(801) 555-0100"))
        assertEquals("8015550100", CallRules.contactNumber(null, "(801) 555.0100"))
        assertEquals("+447700900123", CallRules.contactNumber("not e164", "+44 7700 900123"))
        assertNull(CallRules.contactNumber(null, "ext. office"))
        assertNull(CallRules.contactNumber(null, "12"))
        assertNull(CallRules.contactNumber(null, null))
    }

    /** §9.2: the contacts are searched for two characters or more, and asked for once, at the first search. */
    @Test
    fun theContactsAreAskedForOnceAtTheFirstSearch() {
        assertFalse(CallRules.contactSearchable("a"))
        assertTrue(CallRules.contactSearchable(" an "))
        assertFalse(CallRules.contactSearchable("x".repeat(101)))
        assertTrue(CallRules.askContacts(granted = false, askedBefore = false))
        assertFalse(CallRules.askContacts(granted = false, askedBefore = true))
        assertFalse(CallRules.askContacts(granted = true, askedBefore = false))
    }
}
