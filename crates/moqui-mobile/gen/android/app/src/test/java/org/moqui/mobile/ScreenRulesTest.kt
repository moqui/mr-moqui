// This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License.

package org.moqui.mobile

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class ScreenRulesTest {
    @Test
    fun theCaptureKeepsTheScreensAspectWithinTheLongSideAndEven() {
        assertEquals(Pair(576, 1280), ScreenRules.captureSize(1080, 2400))
        assertEquals(Pair(1280, 576), ScreenRules.captureSize(2400, 1080))
        assertEquals(Pair(720, 1280), ScreenRules.captureSize(1080, 1920))
        // a small screen is not scaled up; an odd side is made even
        assertEquals(Pair(480, 800), ScreenRules.captureSize(481, 800))
        assertEquals(Pair(ScreenRules.MIN_SIDE, ScreenRules.MIN_SIDE), ScreenRules.captureSize(0, 0))
    }

    @Test
    fun aRotationIsANewSize() {
        val portrait = ScreenRules.captureSize(1080, 2400)
        assertTrue(ScreenRules.resized(portrait, 2400, 1080))
        assertFalse(ScreenRules.resized(portrait, 1080, 2400))
        assertTrue(ScreenRules.resized(null, 1080, 2400))
    }

    @Test
    fun theEncoderIsGivenWhatItTakes() {
        assertEquals(1_500_000, ScreenRules.encoderBitrate(1_500_000, 1, 40_000_000))
        assertEquals(1_000_000, ScreenRules.encoderBitrate(2_500_000, 1, 1_000_000))
        assertEquals(200_000, ScreenRules.encoderBitrate(150_000, 200_000, null))
        assertEquals(150_000, ScreenRules.encoderBitrate(150_000, null, null))
    }
}
