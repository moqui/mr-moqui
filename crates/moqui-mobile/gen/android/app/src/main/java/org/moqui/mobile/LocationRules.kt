// This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License.

package org.moqui.mobile

import kotlin.math.asin
import kotlin.math.cos
import kotlin.math.min
import kotlin.math.sin
import kotlin.math.sqrt

/**
 * The live location service's rules (plans/Location-Sharing.md §3.4; [LocationShareService]), apart from Android so
 * the JVM tests check them: how often the phone's location is asked for, when a share stops being sent from here, which
 * of Android's providers is read, and the notification's words.
 */
object LocationRules {
    /** Moving: a fix every few seconds, at least the share's own interval (location@min-interval-seconds). */
    const val MOVING_MS = 5_000L
    /** Still: a fix every half minute, so a still phone spends little battery and the audience still sees it is fresh. */
    const val STILL_MS = 30_000L
    /** Faster than this (metres a second) is moving: a walk is about 1.4. */
    const val MOVING_SPEED = 0.8
    /** Farther than this from the last fix (metres) is moving, for a fix with no speed. */
    const val MOVING_METRES = 25.0

    /**
     * The interval asked of Android's LocationManager for one share: an approximate share's own interval
     * (location@approximate-interval-seconds, which the server takes at most), an exact one's at least [MOVING_MS] while
     * moving and [STILL_MS] while still, never under the share's own.
     */
    fun intervalMs(approximate: Boolean, shareIntervalMs: Long, moving: Boolean): Long {
        val own = shareIntervalMs.coerceAtLeast(1_000L)
        if (approximate) return own
        return maxOf(own, if (moving) MOVING_MS else STILL_MS)
    }

    /** The interval for the shares sent (`approximate` to their interval): the shortest of theirs. */
    fun serviceIntervalMs(shares: List<Pair<Boolean, Long>>, moving: Boolean): Long =
        shares.minOfOrNull { (approximate, interval) -> intervalMs(approximate, interval, moving) } ?: STILL_MS

    /** Whether the phone moves: its speed, else the distance from the last fix. */
    fun moving(speed: Double?, metresFromLast: Double?): Boolean = when {
        speed != null -> speed >= MOVING_SPEED
        metresFromLast != null -> metresFromLast >= MOVING_METRES
        else -> true
    }

    /** The distance between two points in metres (haversine, on a sphere of the Earth's mean radius). */
    fun metres(lat1: Double, lon1: Double, lat2: Double, lon2: Double): Double {
        val r = 6_371_000.0
        val p1 = Math.toRadians(lat1)
        val p2 = Math.toRadians(lat2)
        val dp = p2 - p1
        val dl = Math.toRadians(lon2 - lon1)
        val a = sin(dp / 2) * sin(dp / 2) + cos(p1) * cos(p2) * sin(dl / 2) * sin(dl / 2)
        return 2 * r * asin(min(1.0, sqrt(a)))
    }

    /**
     * Why a share is no longer sent from here at `nowMs`, or null to go on: `ended` at its end, `permission` once the
     * location permission is gone, `signedOut` for an account not signed in here, else the reason of a report's
     * outcome that stopped it (`ended` on a 410, `notFound` on a 404, `otherDevice` when another device sends it,
     * `signedOut`). The notification's Stop and the page's are the user's, not this.
     */
    fun stopReason(thruMs: Long, nowMs: Long, permitted: Boolean, signedIn: Boolean, outcomeState: String?, outcomeReason: String?): String? =
        when {
            nowMs >= thruMs -> "ended"
            !permitted -> "permission"
            !signedIn -> "signedOut"
            outcomeState == "stopped" -> outcomeReason?.takeIf { it.isNotBlank() } ?: "ended"
            else -> null
        }

    /** Whether the service may start: only from the app in front, with the location permission and its notification. */
    fun mayStart(foreground: Boolean, permitted: Boolean, notificationsShown: Boolean): Boolean =
        foreground && permitted && notificationsShown

    /**
     * The provider read: the platform's fused one where Android has it (12+; no Google Play services), else GPS for an
     * exact share and the network's for an approximate one, else whichever is on; GPS never without the precise
     * permission (Android refuses it; an approximate share asks for approximate alone); null with none.
     */
    fun provider(exact: Boolean, fine: Boolean, fused: Boolean, gps: Boolean, network: Boolean): String? = when {
        fused -> "fused"
        exact && fine && gps -> "gps"
        network -> "network"
        fine && gps -> "gps"
        else -> null
    }

    /**
     * The notification's text: "Sharing your location with Sam and 2 others until 15:30" for one share; for more, with
     * the first's audience and the latest end. `until` is the end as the phone shows times.
     */
    fun notificationText(audiences: List<String>, until: String): String {
        val named = audiences.map { it.trim() }.filter { it.isNotEmpty() }
        val whom = when {
            named.isEmpty() -> "the people you chose"
            named.size == 1 -> named[0]
            else -> "${named[0]} and ${named.size - 1} more ${if (named.size == 2) "share" else "shares"}"
        }
        return "Sharing your location with $whom until $until"
    }

    /**
     * The permissions an [LocationShare.allow] asks for (`fine`, `coarse`), none when it asks nothing: an approximate
     * share asks for approximate alone, so where precise was never given the phone's own fixes are approximate; an exact
     * share asks for precise (and approximate, which the person may give instead) unless precise is given or was asked
     * for once already (the person chose approximate: asked again only from Android's settings).
     */
    fun permissionsToAsk(exact: Boolean, fine: Boolean, coarse: Boolean, askedFine: Boolean): List<String> = when {
        fine -> emptyList()
        exact && !askedFine -> listOf("fine", "coarse")
        coarse -> emptyList()
        else -> listOf("coarse")
    }

    /** The accuracy asked of Android (API 31+ `LocationRequest`): high for an exact share, balanced for approximate. */
    fun quality(exact: Boolean): String = if (exact) "high" else "balanced"

    /**
     * The periodic check of the notification: with notifications off nothing is sent (`stop`, and the shares are
     * stopped on their servers), else it is posted again (`repost`: one the person dismissed comes back).
     */
    fun notificationCheck(notificationsShown: Boolean): String = if (notificationsShown) "repost" else "stop"

    /**
     * While a report of a share is under way, the newest fix that comes for it waits in `queued`, replacing an older
     * one; an empty fix (a point that waited, which Rust keeps) never replaces a real one. One queued a share at most.
     */
    fun queueFix(queued: MutableMap<String, String>, key: String, fix: String) {
        if (fix.isNotEmpty() || !queued.containsKey(key)) queued[key] = fix
    }

    /** A share's key, `account/shareId` (a share id has no `/`). */
    fun stopKey(account: String, shareId: String): String = "$account/$shareId"

    /** The account and share of a [stopKey], or null. */
    fun parseStopKey(key: String): Pair<String, String>? {
        val at = key.lastIndexOf('/')
        if (at <= 0 || at == key.length - 1) return null
        return key.substring(0, at) to key.substring(at + 1)
    }

    /**
     * Whether a stop on the server is over: `stopped`, `gone` (no such share there) or `signedOut` (nothing can be
     * told); `retry` and anything else is tried again.
     */
    fun stopSettled(state: String): Boolean = state == "stopped" || state == "gone" || state == "signedOut"

    /**
     * What a stop of a share ending at `thruMs` comes to at `nowMs` when its try came to `state`: `settled` (the server
     * stopped it or has no such share, or its end has passed, when the server ends it anyway), `unconfirmed` (let go
     * before its end with the server not told: the account signed out here; the person is told until when the share
     * shows the last position) or `keep` (tried again).
     */
    fun stopOutcome(state: String, thruMs: Long, nowMs: Long): String = when {
        state == "stopped" || state == "gone" -> "settled"
        nowMs >= thruMs -> "settled"
        state == "signedOut" -> "unconfirmed"
        else -> "keep"
    }

    /**
     * Whether the permission question let a share go on: only when the person went on to Android's question (not Not now,
     * nor a dismissed dialog) and the location may be read now.
     */
    fun allowed(asked: Boolean, permitted: Boolean): Boolean = asked && permitted

    /** How long the confirmation's Send stays disabled after the dialog has the window's focus (tapjacking). */
    const val SEND_DELAY_MS = 1_000L

    /** The longest audience the confirmation names, in code points; more is cut with an ellipsis. */
    const val WHOM_MAX = 400

    /**
     * Whom the confirmation names: the server's words on one line (control characters and line breaks as spaces, runs
     * of spaces as one, and format characters dropped: bidi overrides and isolates, zero-width spaces and joiners, which
     * a name others chose could use to reorder or hide what the dialog shows), at most [WHOM_MAX] characters, or null for
     * none (the dialog then says "the people you chose").
     */
    fun confirmWhom(audience: String): String? {
        // by code point, so the format characters outside the BMP (the tags, U+E0001 and U+E0020 to U+E007F) go too
        val kept = StringBuilder()
        audience.codePoints().forEach { point ->
            when {
                Character.getType(point) == Character.FORMAT.toInt() -> {}
                Character.isISOControl(point) || Character.isWhitespace(point) || Character.isSpaceChar(point) -> kept.append(' ')
                else -> kept.appendCodePoint(point)
            }
        }
        val line = kept.toString().replace(Regex(" +"), " ").trim()
        if (line.isEmpty()) return null
        // by code point, so a character outside the BMP is never cut in half
        val points = line.codePointCount(0, line.length)
        if (points <= WHOM_MAX) return line
        return line.substring(0, line.offsetByCodePoints(0, WHOM_MAX - 1)).trimEnd() + "…"
    }

    /**
     * Whether a touch on Send is refused: one whose `flags` say it came through another app's window, wholly
     * (`FLAG_WINDOW_IS_OBSCURED`) or in part (`FLAG_WINDOW_IS_PARTIALLY_OBSCURED`); other flags do not matter.
     */
    fun obscured(flags: Int): Boolean =
        flags and (android.view.MotionEvent.FLAG_WINDOW_IS_OBSCURED or android.view.MotionEvent.FLAG_WINDOW_IS_PARTIALLY_OBSCURED) != 0

    /**
     * What a paused share's read again comes to (`Native.readLocationShare`'s `state`): `ok` with the share, `gone`
     * (the server has it no longer as this device's, or the account is signed out: let go), else `retry` (kept paused:
     * no network, no client, no answer, or an answer with no share).
     */
    fun readOutcome(state: String, hasShare: Boolean): String = when {
        state == "ok" && hasShare -> "ok"
        state == "gone" -> "gone"
        else -> "retry"
    }

    /**
     * The end as the phone words it when the server gave none: "15:30" today, "Fri 09:00" another day (the server's
     * `untilWords` does the same in the owner's zone).
     */
    fun untilFallback(thruMs: Long, nowMs: Long, zone: java.util.TimeZone = java.util.TimeZone.getDefault()): String {
        val day = java.text.SimpleDateFormat("yyyy-MM-dd", java.util.Locale.ROOT).apply { timeZone = zone }
        val sameDay = day.format(java.util.Date(thruMs)) == day.format(java.util.Date(nowMs))
        val format = java.text.SimpleDateFormat(if (sameDay) "HH:mm" else "EEE HH:mm", java.util.Locale.getDefault())
        format.timeZone = zone
        return format.format(java.util.Date(thruMs))
    }

    /** Whether the share starts after the question: Send, with the app still in front. */
    fun confirmed(send: Boolean, foreground: Boolean): Boolean = send && foreground
}
