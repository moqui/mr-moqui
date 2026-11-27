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
        assertEquals("fused", LocationRules.provider(true, true, true, true))
        assertEquals("gps", LocationRules.provider(true, false, true, true))
        assertEquals("network", LocationRules.provider(false, false, true, true))
        assertEquals("gps", LocationRules.provider(false, false, true, false))
        assertNull(LocationRules.provider(true, false, false, false))
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
}
