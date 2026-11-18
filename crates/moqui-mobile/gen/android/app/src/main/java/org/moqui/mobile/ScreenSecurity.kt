// This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License.

package org.moqui.mobile

import android.content.Context
import android.os.Handler
import android.os.Looper
import android.view.WindowManager
import java.lang.ref.WeakReference

/**
 * Screen security (`FLAG_SECURE`: no screenshots, no screen recording, a blank recent-apps thumbnail), held per reason
 * (plans/Comms-Messaging.md §18.3 phase 16): `viewOnce` while a view-once photo or video is open; D2g's reasons (the app,
 * an encrypted room) join the same set. The flag is on while any reason holds it, on the app's window, and is set again
 * when the activity is made again. The page asks through `moquiDesktop.secureScreen` (the `secure_screen` command).
 * Phase 21's reasons are `room:<id>` while such a room is shown and `app` for the whole app; `app` is remembered on the
 * phone, so from the next start the flag is on before the first frame (the splash and the account list too).
 */
object ScreenSecurity {
    private val reasons = mutableSetOf<String>()
    private var screen: WeakReference<MainActivity>? = null
    private val main = Handler(Looper.getMainLooper())

    /** The activity shown now: the flag follows the reasons held. */
    fun attach(activity: MainActivity) {
        screen = WeakReference(activity)
        if (remembered(activity)) synchronized(reasons) { reasons.add(APP) }
        apply()
    }

    private const val APP = "app"
    private const val PREFS = "moqui-screen-security"

    private fun remembered(context: Context): Boolean =
        context.getSharedPreferences(PREFS, Context.MODE_PRIVATE).getBoolean(APP, false)

    private fun remember(on: Boolean) {
        val activity = screen?.get() ?: return
        activity.getSharedPreferences(PREFS, Context.MODE_PRIVATE).edit().putBoolean(APP, on).apply()
    }

    /** A reason on or off; whether the flag is on now. */
    @JvmStatic
    fun set(reason: String, on: Boolean): Boolean {
        val held = synchronized(reasons) {
            if (on) reasons.add(reason) else reasons.remove(reason)
            reasons.isNotEmpty()
        }
        if (reason == APP) remember(on)
        apply()
        return held
    }

    private fun apply() {
        val held = synchronized(reasons) { reasons.isNotEmpty() }
        main.post {
            val window = screen?.get()?.window ?: return@post
            if (held) window.addFlags(WindowManager.LayoutParams.FLAG_SECURE)
            else window.clearFlags(WindowManager.LayoutParams.FLAG_SECURE)
        }
    }
}
