// This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License.

package org.moqui.mobile

import android.app.AlertDialog
import android.view.View

/**
 * The overlay defence below Android 12 (SEC-CLI-116): `minSdk` is 26, and Android drops the touches that pass through
 * an untrusted overlay itself only from 12, so before that an app with the overlay permission could cover a call's
 * Answer, a permission question or a dialog's button and pass the user's taps through. A touch that came through
 * another app's window is dropped: on each activity's decor view (`ObscuredTouches.refuse(window.decorView)` in its
 * `onCreate`), whose filter drops it before any child sees it, and on each dialog's own window once shown ([show]).
 * wry's view and the page's dialogs do the same (`RustWebView.refuseObscuredTouches`, crates/moqui-mobile/patches).
 */
object ObscuredTouches {
    /** `view` (an activity's or a shown dialog's decor view) drops a touch while another app's window covers it. */
    fun refuse(view: View?) {
        view?.filterTouchesWhenObscured = true
    }

    /** `dialog` shown, its window refusing obscured touches: every dialog of the app is shown by this. */
    fun show(dialog: AlertDialog.Builder): AlertDialog {
        val shown = dialog.show()
        refuse(shown.window?.decorView)
        return shown
    }

    /**
     * `dialog` created, then `prepare`d (its window's flags, set before it shows: hidden overlays), then shown with its
     * window refusing obscured touches.
     */
    fun show(dialog: AlertDialog.Builder, prepare: (AlertDialog) -> Unit): AlertDialog {
        val created = dialog.create()
        prepare(created)
        created.show()
        refuse(created.window?.decorView)
        return created
    }
}
