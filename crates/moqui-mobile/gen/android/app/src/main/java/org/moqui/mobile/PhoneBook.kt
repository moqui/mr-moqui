// This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License.

package org.moqui.mobile

import android.Manifest
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.net.Uri
import android.os.Handler
import android.os.Looper
import android.provider.ContactsContract
import android.provider.Settings
import android.util.Log
import org.json.JSONArray
import org.json.JSONObject
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit

/**
 * The phone's own contacts (plans/Comms-Messaging.md §9.2, Phone book): the names of outside numbers on phone rooms
 * ([names]) and the people the new-chat picker finds as the user types ([search]). Read on the phone only, for the
 * shell's page (`contact_names`, `contacts_search` in src/commands.rs, through JNI): nothing is sent to a server or kept.
 *
 * The permission is asked on first use, a search, with the reason first ([askOnce]); a phone room's names never ask (a
 * room list opened is not a use), and without it they come from the server's apps (`find#PhoneNames`) only. Once asked
 * the app does not ask again by itself: the notification check's Contacts row asks on the user's tap ([fix]).
 */
object PhoneBook {
    private const val TAG = "moqui-contacts"
    private const val PREFS = "moqui-contacts"
    const val ASK_CONTACTS = 3
    /** The longest the search waits for the user's answer to the permission's question. */
    private const val ASK_WAIT_S = 60L
    private const val SEARCH_MAX = 20
    private const val NAMES_MAX = 50

    @Volatile private var answer: CountDownLatch? = null

    fun granted(context: Context): Boolean =
        context.checkSelfPermission(Manifest.permission.READ_CONTACTS) == PackageManager.PERMISSION_GRANTED

    private fun askedBefore(context: Context): Boolean =
        context.getSharedPreferences(PREFS, Context.MODE_PRIVATE).getBoolean("asked", false)

    /**
     * The contacts whose name or number has [term], as JSON `[{name, number}]` (at most [limit], 20); none without the
     * permission. A first search asks for it (the reason, then the system's question) and waits for the answer. Not on
     * the main thread.
     */
    @JvmStatic
    fun search(context: Context, term: String, limit: Int): String {
        val app = context.applicationContext
        if (!CallRules.contactSearchable(term)) return "[]"
        if (!granted(app) && !(CallRules.askContacts(false, askedBefore(app)) && askOnce(app))) return "[]"
        val found = JSONArray()
        val seen = HashSet<String>()
        val uri = Uri.withAppendedPath(ContactsContract.CommonDataKinds.Phone.CONTENT_FILTER_URI, Uri.encode(term.trim()))
        val columns = arrayOf(
            ContactsContract.CommonDataKinds.Phone.DISPLAY_NAME_PRIMARY,
            ContactsContract.CommonDataKinds.Phone.NUMBER,
            ContactsContract.CommonDataKinds.Phone.NORMALIZED_NUMBER,
        )
        try {
            app.contentResolver.query(uri, columns, null, null, null)?.use { rows ->
                while (rows.moveToNext() && found.length() < limit.coerceIn(1, SEARCH_MAX)) {
                    val number = CallRules.contactNumber(rows.getString(2), rows.getString(1)) ?: continue
                    if (!seen.add(number)) continue
                    found.put(JSONObject().put("name", rows.getString(0)?.take(200) ?: "").put("number", number))
                }
            }
        } catch (err: Exception) {
            Log.w(TAG, "The contacts were not searched: ${err.message}")
        }
        return found.toString()
    }

    /**
     * The names of [numbers] (newline-separated, E.164) the phone's contacts have, as JSON `{number: name}`; at most 50,
     * and none without the permission (never asked here). Not on the main thread.
     */
    @JvmStatic
    fun names(context: Context, numbers: String): String {
        val app = context.applicationContext
        val named = JSONObject()
        if (!granted(app)) return named.toString()
        for (number in numbers.split('\n').map { it.trim() }.filter { it.isNotEmpty() }.distinct().take(NAMES_MAX)) {
            val uri = Uri.withAppendedPath(ContactsContract.PhoneLookup.CONTENT_FILTER_URI, Uri.encode(number))
            try {
                app.contentResolver.query(uri, arrayOf(ContactsContract.PhoneLookup.DISPLAY_NAME), null, null, null)?.use { rows ->
                    if (rows.moveToFirst()) rows.getString(0)?.takeIf { it.isNotBlank() }?.let { named.put(number, it.take(200)) }
                }
            } catch (err: Exception) {
                Log.w(TAG, "A number's name was not looked up: ${err.message}")
            }
        }
        return named.toString()
    }

    /** The first use: the reason, then the system's question, once; waits for the answer. Whether it was granted. */
    private fun askOnce(context: Context): Boolean {
        val activity = CallManager.appActivity() ?: return false
        context.getSharedPreferences(PREFS, Context.MODE_PRIVATE).edit().putBoolean("asked", true).apply()
        val waiting = CountDownLatch(1)
        answer = waiting
        Handler(Looper.getMainLooper()).post {
            if (activity.isFinishing || activity.isDestroyed) {
                waiting.countDown()
                return@post
            }
            var asked = false
            android.app.AlertDialog.Builder(activity)
                .setTitle(R.string.ask_contacts_title)
                .setMessage(R.string.ask_contacts_reason)
                .setPositiveButton(R.string.ask_continue) { _, _ ->
                    asked = true
                    activity.requestPermissions(arrayOf(Manifest.permission.READ_CONTACTS), ASK_CONTACTS)
                }
                .setNegativeButton(R.string.ask_not_now, null)
                .setOnDismissListener { if (!asked) waiting.countDown() }
                .show()
        }
        waiting.await(ASK_WAIT_S, TimeUnit.SECONDS)
        answer = null
        return granted(context)
    }

    /** The system's question was answered ([MainActivity.onRequestPermissionsResult]). */
    fun answered() {
        answer?.countDown()
    }

    /**
     * The notification check's fix: the system's question again where Android still asks it, else the app's settings,
     * where the user allows the contacts.
     */
    fun fix(context: Context) {
        val activity = CallManager.appActivity()
        val stillAsks = activity != null && (!askedBefore(context) ||
            activity.shouldShowRequestPermissionRationale(Manifest.permission.READ_CONTACTS))
        context.getSharedPreferences(PREFS, Context.MODE_PRIVATE).edit().putBoolean("asked", true).apply()
        if (activity != null && stillAsks) {
            Handler(Looper.getMainLooper()).post {
                activity.requestPermissions(arrayOf(Manifest.permission.READ_CONTACTS), ASK_CONTACTS)
            }
            return
        }
        try {
            context.startActivity(
                Intent(Settings.ACTION_APPLICATION_DETAILS_SETTINGS, Uri.parse("package:${context.packageName}"))
                    .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
            )
        } catch (err: Exception) {
            Log.w(TAG, "The app's settings did not open: ${err.message}")
        }
    }
}
