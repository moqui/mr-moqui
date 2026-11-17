// This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License.

package org.moqui.mobile

import android.content.Context
import android.telecom.Connection
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Test
import org.junit.runner.RunWith

/**
 * The app's calls in Telecom on a device (plans/Comms-Messaging.md §8.3 items 5 and 7; instrumented, run on request on
 * the emulator, as docs/ui/Mobile-Client.md#tests says): a call the page joins by itself becomes a self-managed Telecom
 * call; the page's `held` holds it and `joined` resumes it; its audio goes to the earpiece for voice and the speaker for
 * video, and the page's choice moves it; leaving ends it.
 */
@RunWith(AndroidJUnit4::class)
class CallTelecomTest {
    private lateinit var context: Context

    @Before
    fun setUp() {
        context = InstrumentationRegistry.getInstrumentation().targetContext
        CallManager.know(context)
    }

    @After
    fun tearDown() {
        for (id in listOf("T1", "T2")) CallManager.pageCallState(id, "left")
        waitFor { CallManager.calls().isEmpty() }
    }

    private fun waitFor(what: () -> Boolean) {
        val until = System.currentTimeMillis() + 8_000
        while (!what() && System.currentTimeMillis() < until) Thread.sleep(50)
    }

    private fun connection(id: String): CallConnection? = CallManager.call(id)?.connection

    @Test
    fun aJoinedCallIsInTelecomAndItsHoldIsTelecomsHold() {
        CallManager.pageCallState("T1", "joined")
        waitFor { connection("T1")?.state == Connection.STATE_ACTIVE }
        val telecom = connection("T1")
        assertNotNull("the joined call is a Telecom call", telecom)
        assertEquals(Connection.STATE_ACTIVE, telecom!!.state)
        assertTrue("it can be held", telecom.connectionCapabilities and Connection.CAPABILITY_HOLD != 0)
        // call waiting: the page's hold is Telecom's hold, and joined again resumes it
        CallManager.pageCallState("T1", "held")
        waitFor { telecom.state == Connection.STATE_HOLDING }
        assertEquals(Connection.STATE_HOLDING, telecom.state)
        assertNull("a held call has no audio routed", CallManager.activeConnection())
        CallManager.pageCallState("T1", "joined")
        waitFor { telecom.state == Connection.STATE_ACTIVE }
        assertEquals(Connection.STATE_ACTIVE, telecom.state)
        CallManager.pageCallState("T1", "left")
        waitFor { telecom.state == Connection.STATE_DISCONNECTED }
        assertEquals(Connection.STATE_DISCONNECTED, telecom.state)
        assertNull(CallManager.call("T1"))
    }

    /**
     * Review findings: a call's kind that comes before its joined is kept for it; the call screen's screen counts for
     * picture-in-picture whichever call it shows; the page gone (a server page, a reload, an account switch) ends the
     * calls it joined; the notification's Hang up ends a native call at once, one on hold too.
     */
    @Test
    fun aKindBeforeJoinedTheScreenAndThePageGoneOrHangUpEndTheNativeCall() {
        CallManager.pageCallState("T1", "video")
        CallManager.pageCallState("T1", "screen")
        CallManager.pageCallState("T1", "joined", "Ann Example")
        waitFor { connection("T1")?.state == Connection.STATE_ACTIVE }
        assertTrue("the kind that came first", CallManager.call("T1")!!.video)
        assertEquals("Ann Example", CallManager.call("T1")!!.caller)
        assertEquals("Ann Example", connection("T1")!!.callerDisplayName)
        waitFor { CallManager.pipWanted() }
        assertTrue("a video call on the call screen goes to picture-in-picture", CallManager.pipWanted())
        CallManager.pageCallState("T1", "offScreen")
        waitFor { !CallManager.pipWanted() }
        assertFalse(CallManager.pipWanted())
        // the page goes: the call it joined ends here
        val telecom = connection("T1")!!
        CallManager.pageGone()
        waitFor { telecom.state == Connection.STATE_DISCONNECTED }
        assertEquals(Connection.STATE_DISCONNECTED, telecom.state)
        assertNull(CallManager.call("T1"))
        // a held call: the notification's Hang up ends it here at once
        CallManager.pageCallState("T2", "joined")
        waitFor { connection("T2")?.state == Connection.STATE_ACTIVE }
        CallManager.pageCallState("T2", "held")
        val held = connection("T2")!!
        waitFor { held.state == Connection.STATE_HOLDING }
        InstrumentationRegistry.getInstrumentation().runOnMainSync { CallManager.hangUp(context, "T2") }
        waitFor { held.state == Connection.STATE_DISCONNECTED }
        assertEquals(Connection.STATE_DISCONNECTED, held.state)
        assertNull(CallManager.call("T2"))
    }

    /**
     * The second review of phase 4: while one call is held the call the app is in cannot be held by Telecom (the page
     * would refuse), and can again after; an older call screen's offScreen after a newer one's screen leaves it shown.
     */
    @Test
    fun aCallIsNotHoldableWhileAnotherIsHeldAndAnOlderScreenGoesUnheard() {
        CallManager.pageCallState("T1", "joined")
        waitFor { connection("T1")?.state == Connection.STATE_ACTIVE }
        val first = connection("T1")!!
        assertTrue(first.connectionCapabilities and Connection.CAPABILITY_HOLD != 0)
        CallManager.pageCallState("T1", "held")
        waitFor { first.state == Connection.STATE_HOLDING }
        CallManager.pageCallState("T2", "joined")
        waitFor { connection("T2")?.state == Connection.STATE_ACTIVE }
        val second = connection("T2")!!
        waitFor { second.connectionCapabilities and Connection.CAPABILITY_HOLD == 0 }
        assertEquals("the call in use, with another held: no hold", 0, second.connectionCapabilities and Connection.CAPABILITY_HOLD)
        assertTrue("the held one keeps it", first.connectionCapabilities and Connection.CAPABILITY_HOLD != 0)
        // the second call left: the held one back (Resume) can be held again
        CallManager.pageCallState("T2", "left")
        CallManager.pageCallState("T1", "joined")
        waitFor { first.state == Connection.STATE_ACTIVE }
        waitFor { first.connectionCapabilities and Connection.CAPABILITY_HOLD != 0 }
        assertTrue(first.connectionCapabilities and Connection.CAPABILITY_HOLD != 0)
        // the call screen: a newer mount's screen, then the older one's offScreen: still shown
        CallManager.pageCallState("T1", "screen", "20")
        CallManager.pageCallState("T1", "offScreen", "10")
        waitFor { CallManager.screenShown }
        Thread.sleep(200)
        assertTrue("an older screen's offScreen is dropped", CallManager.screenShown)
        CallManager.pageCallState("T1", "offScreen", "20")
        waitFor { !CallManager.screenShown }
        assertFalse(CallManager.screenShown)
    }

    @Test
    fun aVoiceCallGoesToTheEarpieceAVideoCallToTheSpeakerAndThePageChooses() {
        CallManager.pageCallState("T2", "joined")
        CallManager.pageCallState("T2", "voice")
        waitFor { CallAudio.routes.isNotEmpty() && CallAudio.route != null }
        val routes = CallAudio.routes
        assertTrue("Telecom gives the routes: $routes", routes.contains(CallRules.SPEAKER))
        if (CallRules.EARPIECE in routes) {
            waitFor { CallAudio.route == CallRules.EARPIECE }
            assertEquals(CallRules.EARPIECE, CallAudio.route)
        }
        CallManager.pageCallState("T2", "video")
        waitFor { CallAudio.route == CallRules.SPEAKER }
        assertEquals("a video call: the speaker", CallRules.SPEAKER, CallAudio.route)
        assertFalse("no screen off at the ear in a video call", CallAudio.proximityHeld())
        if (CallRules.EARPIECE in routes) {
            assertTrue(CallAudio.request(CallRules.EARPIECE))
            waitFor { CallAudio.route == CallRules.EARPIECE }
            assertEquals("the page's choice", CallRules.EARPIECE, CallAudio.route)
            // the user's choice holds when the call's kind changes
            CallManager.pageCallState("T2", "voice")
            CallManager.pageCallState("T2", "video")
            Thread.sleep(500)
            assertEquals(CallRules.EARPIECE, CallAudio.route)
        }
        assertFalse("no such route", CallAudio.request("satellite"))
        assertTrue(CallAudio.state().contains("\"routes\""))
    }
}
