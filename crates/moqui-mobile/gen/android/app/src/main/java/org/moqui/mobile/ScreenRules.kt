// This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License.

package org.moqui.mobile

/**
 * The native screen share's rules (`ScreenShare`), apart from Android so the JVM tests check them: the capture's size
 * from the screen's, and the bitrate the encoder is given.
 */
object ScreenRules {
    /** The long side of the capture, in pixels: enough for a phone's text at a viewer's tile size. */
    const val MAX_SIDE = 1280

    /** The smallest side kept: a narrower capture is not worth sending. */
    const val MIN_SIDE = 64

    /**
     * The capture's width and height for a screen of `width` x `height`: its aspect kept, the long side at most
     * [MAX_SIDE], both even (VP8 encoders take even sides) and at least [MIN_SIDE]. A portrait screen stays portrait, so a
     * rotation gives a new size, and the encoder starts again at it.
     */
    fun captureSize(width: Int, height: Int, maxSide: Int = MAX_SIDE): Pair<Int, Int> {
        if (width <= 0 || height <= 0) return Pair(MIN_SIDE, MIN_SIDE)
        val scale = minOf(1.0, maxSide.toDouble() / maxOf(width, height))
        fun side(v: Int) = maxOf(MIN_SIDE, (v * scale).toInt() and 1.inv())
        return Pair(side(width), side(height))
    }

    /** The bitrate the encoder is given: Rust's ask, within what the encoder says it takes (its `bitrateRange`). */
    fun encoderBitrate(asked: Int, lower: Int?, upper: Int?): Int {
        var bps = asked
        if (upper != null && upper > 0) bps = minOf(bps, upper)
        if (lower != null && lower > 0) bps = maxOf(bps, lower)
        return bps
    }

    /** Whether a new screen size needs the encoder again: the capture's size changed. */
    fun resized(current: Pair<Int, Int>?, width: Int, height: Int): Boolean =
        current != captureSize(width, height)
}
