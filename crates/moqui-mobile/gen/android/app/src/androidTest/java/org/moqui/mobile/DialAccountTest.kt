// This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License.

package org.moqui.mobile

import android.content.Context
import android.content.Intent
import android.net.Uri
import android.os.Bundle
import android.telecom.Connection
import android.telecom.ConnectionRequest
import android.telecom.DisconnectCause
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import org.json.JSONArray
import org.json.JSONObject
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Test
import org.junit.runner.RunWith

/**
 * The phone's dialer and contacts through the app on a device (plans/Comms-Messaging.md §9.1, §9.2; instrumented, run on
 * request on the emulator, as docs/ui/Mobile-Client.md#tests says): the calling account is registered and taken away,
 * off until the user turns it on; a number dialed with it is handed to the app by a link with a nonce only, Telecom's
 * own call cancelled; a number the app does not call fails; the contacts give names and search results.
 */
@RunWith(AndroidJUnit4::class)
class DialAccountTest {
    private lateinit var context: Context
    private val opened = mutableListOf<Intent>()
    private val shell = InstrumentationRegistry.getInstrumentation().uiAutomation

    @Before
    fun setUp() {
        context = InstrumentationRegistry.getInstrumentation().targetContext
        CallManager.know(context)
        DialAccount.opener = { _, intent -> opened.add(intent) }
    }

    @After
    fun tearDown() {
        DialAccount.opener = { context, intent -> context.startActivity(intent) }
        DialAccount.set(context, false)
        DialAccount.handedOver(context)
    }

    private fun request(number: String) = ConnectionRequest(DialAccount.handle(context), Uri.fromParts("tel", number, null), Bundle())

    @Test
    fun theCallingAccountIsRegisteredOnOnceUsedAndTakenAway() {
        assertTrue("Telecom took the account", DialAccount.set(context, true))
        assertEquals("registered, maybe not turned on yet", "registered", DialAccount.state(context))
        DialAccount.dialed(context, request("8015550100"))
        assertEquals("a number came through it: on", "on", DialAccount.state(context))
        assertTrue(DialAccount.set(context, false))
        assertEquals("taken away", "off", DialAccount.state(context))
        assertTrue(DialAccount.set(context, true))
        assertEquals("registered again: not known to be on", "registered", DialAccount.state(context))
    }

    @Test
    fun aDialedNumberGoesToTheAppByANonceOnlyAndTelecomsCallIsCancelled() {
        val handed = DialAccount.dialed(context, request("+1 801 555 0100"))
        assertEquals(Connection.STATE_DISCONNECTED, handed.state)
        assertEquals(DisconnectCause.CANCELED, handed.disconnectCause.code)
        assertEquals(1, opened.size)
        val link = opened[0].data!!
        assertEquals("org.moqui.mobile", link.scheme)
        assertEquals("/dial", link.path)
        assertTrue("a nonce", !link.getQueryParameter("nonce").isNullOrEmpty())
        assertTrue("no number in the link", !link.toString().contains("555"))
        // a pause, an extension or letters: not called, and the app not opened
        val refused = DialAccount.dialed(context, request("1-800-FLOWERS"))
        assertEquals(DisconnectCause.ERROR, refused.disconnectCause.code)
        assertEquals(1, opened.size)
    }

    @Test
    fun theContactsNameANumberAndAreFoundByName() {
        // granted at the install (-g): revoking it would end this process; without it the rules give nothing (CallRulesTest)
        assertTrue(PhoneBook.granted(context))
        // a contact of the device's own, put in through the shell (whose commands are split at spaces: no quotes), found
        // by name and named by number
        run("content insert --uri content://com.android.contacts/raw_contacts --bind account_type:s:moqui.test --bind account_name:s:moqui.test")
        val raw = Regex("_id=(\\d+), account_type=moqui.test").findAll(
            run("content query --uri content://com.android.contacts/raw_contacts --projection _id:account_type")
        ).last().groupValues[1]
        run("content insert --uri content://com.android.contacts/data --bind raw_contact_id:i:$raw " +
            "--bind mimetype:s:vnd.android.cursor.item/name --bind data1:s:Zed-Moquitest")
        run("content insert --uri content://com.android.contacts/data --bind raw_contact_id:i:$raw " +
            "--bind mimetype:s:vnd.android.cursor.item/phone_v2 --bind data1:s:+18015550188 --bind data4:s:+18015550188")
        try {
            val named = waitFor { JSONObject(PhoneBook.names(context, "+18015550188\n+18015550199")).takeIf { it.has("+18015550188") } }
            assertEquals("Zed-Moquitest", named?.optString("+18015550188"))
            assertTrue("an unknown number is not named", named?.has("+18015550199") == false)
            val found = waitFor { JSONArray(PhoneBook.search(context, "Zed-Moq", 5)).takeIf { it.length() > 0 } }
            assertEquals("+18015550188", found?.getJSONObject(0)?.optString("number"))
            assertEquals("Zed-Moquitest", found?.getJSONObject(0)?.optString("name"))
            assertNull("one character searches nothing", JSONArray(PhoneBook.search(context, "Z", 5)).optJSONObject(0))
        } finally {
            run("content delete --uri content://com.android.contacts/raw_contacts/$raw?caller_is_syncadapter=true")
        }
    }

    private fun run(command: String): String {
        val out = shell.executeShellCommand(command)
        return android.os.ParcelFileDescriptor.AutoCloseInputStream(out).bufferedReader().readText()
    }

    private fun <T> waitFor(what: () -> T?): T? {
        val until = System.currentTimeMillis() + 5_000
        while (System.currentTimeMillis() < until) {
            what()?.let { return it }
            Thread.sleep(100)
        }
        return what()
    }
}
