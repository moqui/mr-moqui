// This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License.

package org.moqui.mobile

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/** The live location service's rules (plans/Location-Sharing.md §3.4, LL3): the interval, the stop rules, the start. */
class LocationRulesTest {
    @Test
    fun anExactShareIsAskedEveryFewSecondsMovingAndLessOftenStill() {
        assertEquals(LocationRules.MOVING_MS, LocationRules.intervalMs(false, 5_000L, true))
        assertEquals(LocationRules.STILL_MS, LocationRules.intervalMs(false, 5_000L, false))
        // never under the share's own interval (a server whose min-interval-seconds is longer)
        assertEquals(10_000L, LocationRules.intervalMs(false, 10_000L, true))
        assertEquals(45_000L, LocationRules.intervalMs(false, 45_000L, false))
    }

    @Test
    fun anApproximateShareIsAskedAtItsOwnInterval() {
        assertEquals(60_000L, LocationRules.intervalMs(true, 60_000L, true))
        assertEquals(60_000L, LocationRules.intervalMs(true, 60_000L, false))
        // the shortest of the shares sent: an exact one with an approximate one
        assertEquals(5_000L, LocationRules.serviceIntervalMs(listOf(true to 60_000L, false to 5_000L), true))
        assertEquals(30_000L, LocationRules.serviceIntervalMs(listOf(true to 60_000L, false to 5_000L), false))
        assertEquals(LocationRules.STILL_MS, LocationRules.serviceIntervalMs(emptyList(), true))
    }

    @Test
    fun movingIsBySpeedElseByDistance() {
        assertTrue(LocationRules.moving(1.4, null))
        assertFalse(LocationRules.moving(0.2, 100.0))
        assertTrue(LocationRules.moving(null, 40.0))
        assertFalse(LocationRules.moving(null, 3.0))
        // the first fix: moving until told otherwise
        assertTrue(LocationRules.moving(null, null))
        val metres = LocationRules.metres(40.0, -111.0, 40.001, -111.0)
        assertTrue("about 111 m: $metres", metres > 110 && metres < 112)
    }

    @Test
    fun aShareStopsAtItsEndOnA410OrA404WhenAnotherDeviceSendsItAtASignOutAndWithoutThePermission() {
        val thru = 1_000_000L
        assertNull(LocationRules.stopReason(thru, thru - 1, true, true, "sent", null))
        assertNull(LocationRules.stopReason(thru, thru - 1, true, true, "waiting", null))
        assertNull(LocationRules.stopReason(thru, thru - 1, true, true, "dropped", null))
        assertEquals("ended", LocationRules.stopReason(thru, thru, true, true, null, null))
        assertEquals("permission", LocationRules.stopReason(thru, 0, false, true, null, null))
        assertEquals("signedOut", LocationRules.stopReason(thru, 0, true, false, null, null))
        assertEquals("ended", LocationRules.stopReason(thru, 0, true, true, "stopped", "ended"))
        assertEquals("notFound", LocationRules.stopReason(thru, 0, true, true, "stopped", "notFound"))
        assertEquals("otherDevice", LocationRules.stopReason(thru, 0, true, true, "stopped", "otherDevice"))
        assertEquals("signedOut", LocationRules.stopReason(thru, 0, true, true, "stopped", "signedOut"))
    }

    @Test
    fun theServiceStartsOnlyFromTheAppInFrontWithThePermissionAndItsNotification() {
        assertTrue(LocationRules.mayStart(true, true, true))
        assertFalse(LocationRules.mayStart(false, true, true))
        assertFalse(LocationRules.mayStart(true, false, true))
        assertFalse(LocationRules.mayStart(true, true, false))
    }

    @Test
    fun theProviderIsThePlatformsWithNoPlayServices() {
        assertEquals("fused", LocationRules.provider(true, true, true, true, true))
        assertEquals("gps", LocationRules.provider(true, true, false, true, true))
        assertEquals("network", LocationRules.provider(false, true, false, true, true))
        assertEquals("gps", LocationRules.provider(false, true, false, true, false))
        assertNull(LocationRules.provider(true, true, false, false, false))
    }

    @Test
    fun theNotificationSaysWithWhomAndUntilWhen() {
        assertEquals("Sharing your location with Sam and 2 others until 15:30",
            LocationRules.notificationText(listOf("Sam and 2 others"), "15:30"))
        assertEquals("Sharing your location with Sam and 1 more share until 18:00",
            LocationRules.notificationText(listOf("Sam", "Pat"), "18:00"))
        assertEquals("Sharing your location with the people you chose until 9:05",
            LocationRules.notificationText(listOf(""), "9:05"))
    }

    @Test
    fun anApproximateShareAsksForApproximateAloneAndAnExactOneForPreciseOnce() {
        assertEquals(listOf("coarse"), LocationRules.permissionsToAsk(false, false, false, false))
        assertEquals(emptyList<String>(), LocationRules.permissionsToAsk(false, false, true, false))
        assertEquals(listOf("fine", "coarse"), LocationRules.permissionsToAsk(true, false, false, false))
        // approximate given already: precise asked once, then not again (the person chose)
        assertEquals(listOf("fine", "coarse"), LocationRules.permissionsToAsk(true, false, true, false))
        assertEquals(emptyList<String>(), LocationRules.permissionsToAsk(true, false, true, true))
        assertEquals(listOf("coarse"), LocationRules.permissionsToAsk(true, false, false, true))
        assertEquals(emptyList<String>(), LocationRules.permissionsToAsk(true, true, true, false))
        assertEquals("high", LocationRules.quality(true))
        assertEquals("balanced", LocationRules.quality(false))
    }

    @Test
    fun theNotificationIsPostedAgainOrTheSharesStopWithNotificationsOff() {
        assertEquals("repost", LocationRules.notificationCheck(true))
        assertEquals("stop", LocationRules.notificationCheck(false))
    }

    @Test
    fun oneFixAShareWaitsWhileItsReportIsUnderWay() {
        val queued = HashMap<String, String>()
        LocationRules.queueFix(queued, "a/1", "fix1")
        LocationRules.queueFix(queued, "a/1", "fix2")
        assertEquals(mapOf("a/1" to "fix2"), queued)
        // a flush (no fix) never takes a real fix's place, and waits itself when none does
        LocationRules.queueFix(queued, "a/1", "")
        assertEquals("fix2", queued["a/1"])
        LocationRules.queueFix(queued, "a/2", "")
        assertEquals("", queued["a/2"])
        assertEquals(2, queued.size)
    }

    @Test
    fun aStopIsKeptUntilItsServerSettlesItOrTheShareEnds() {
        val key = LocationRules.stopKey("chat.example.com-0123", "100")
        assertEquals("chat.example.com-0123" to "100", LocationRules.parseStopKey(key))
        assertNull(LocationRules.parseStopKey("no-share/"))
        assertNull(LocationRules.parseStopKey("/100"))
        val thru = 1_000_000L
        // stopped or no such share: settled; no answer: kept until the share's end, which ends it on the server anyway
        assertEquals("settled", LocationRules.stopOutcome("stopped", thru, 0))
        assertEquals("settled", LocationRules.stopOutcome("gone", thru, 0))
        assertEquals("keep", LocationRules.stopOutcome("retry", thru, thru - 1))
        assertEquals("keep", LocationRules.stopOutcome("", thru, thru - 1))
        assertEquals("settled", LocationRules.stopOutcome("retry", thru, thru))
        // signed out here before its end: let go, the person told until when it shows the last position
        assertEquals("unconfirmed", LocationRules.stopOutcome("signedOut", thru, thru - 1))
        assertEquals("settled", LocationRules.stopOutcome("signedOut", thru, thru + 1))
        assertTrue(LocationRules.stopSettled("gone"))
        assertFalse(LocationRules.stopSettled("retry"))
    }

    @Test
    fun notNowStartsNothingAndTheSendIsThePersons() {
        // the permission question: Not now or a dismissed dialog is no, whatever was given before
        assertFalse(LocationRules.allowed(false, true))
        assertFalse(LocationRules.allowed(true, false))
        assertTrue(LocationRules.allowed(true, true))
        // the app's own question: Send, with the app still in front
        assertTrue(LocationRules.confirmed(true, true))
        assertFalse(LocationRules.confirmed(false, true))
        assertFalse(LocationRules.confirmed(true, false))
        // the server's words on one line, bounded: a name others chose cannot push the end off the dialog
        assertEquals("Sam Smith (sam) and Family (4 members)", LocationRules.confirmWhom(" Sam Smith (sam)\n\tand  Family (4 members) "))
        assertEquals("Sam (sam) and Pat (pat)", LocationRules.confirmWhom("Sam (sam)\u0000and\u202FPat (pat)"))
        assertNull(LocationRules.confirmWhom(" \n "))
        // format characters go: a right-to-left override, an isolate, a zero-width space and joiner
        assertEquals("Sam (sam) and evil (eve)", LocationRules.confirmWhom("Sam (sam) and \u202Eevil\u202C (\u2066eve\u2069\u200B\u200D)"))
        assertNull(LocationRules.confirmWhom("\u200B\u202E"))
        val long = LocationRules.confirmWhom("Sam ".repeat(200))!!
        assertEquals(LocationRules.WHOM_MAX, long.length)
        assertTrue(long.endsWith("…"))
        // a touch through another window, wholly or in part, never presses Send, whatever other flags it has; it waits a
        // second after each focus
        val obscured = android.view.MotionEvent.FLAG_WINDOW_IS_OBSCURED
        val partially = android.view.MotionEvent.FLAG_WINDOW_IS_PARTIALLY_OBSCURED
        assertTrue(LocationRules.obscured(obscured))
        assertTrue(LocationRules.obscured(partially))
        assertTrue(LocationRules.obscured(partially or 0x100))
        assertFalse(LocationRules.obscured(0))
        assertFalse(LocationRules.obscured(0x100))
        assertTrue(LocationRules.SEND_DELAY_MS >= 1_000L)
        // a paused share read again: confirmed only with its share, let go only when the server says so
        assertEquals("ok", LocationRules.readOutcome("ok", true))
        assertEquals("retry", LocationRules.readOutcome("ok", false))
        assertEquals("gone", LocationRules.readOutcome("gone", false))
        assertEquals("retry", LocationRules.readOutcome("retry", false))
        assertEquals("retry", LocationRules.readOutcome("", false))
    }

    @Test
    fun gpsIsNeverReadWithoutThePrecisePermission() {
        assertEquals("network", LocationRules.provider(false, false, false, true, true))
        assertNull(LocationRules.provider(false, false, false, true, false))
        assertNull(LocationRules.provider(true, false, false, true, false))
        assertEquals("gps", LocationRules.provider(true, true, false, true, false))
        assertEquals("fused", LocationRules.provider(false, false, true, true, false))
    }

    @Test
    fun anEndNotTodayNamesItsDay() {
        val utc = java.util.TimeZone.getTimeZone("UTC")
        // 2026-11-27 (a Friday) 10:00 UTC, and the next day's 09:05
        val now = 1_795_773_600_000L
        val today = now + 5 * 3_600_000L
        val tomorrow = now + 23 * 3_600_000L + 5 * 60_000L
        assertEquals("15:00", LocationRules.untilFallback(today, now, utc))
        val next = LocationRules.untilFallback(tomorrow, now, utc)
        assertTrue(next, next.endsWith("09:05") && next.length > 5)
    }
}
