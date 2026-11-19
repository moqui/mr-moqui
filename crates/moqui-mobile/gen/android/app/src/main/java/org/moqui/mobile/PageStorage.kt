// This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License.

package org.moqui.mobile

import android.os.Handler
import android.os.Looper
import android.webkit.WebStorage

/**
 * The storage of the server pages' origin, which every account's pages share in the app's one webview, cleared when
 * the current account changes or signs out (src/host.rs clear_page_storage, called by Rust by name), so one account's
 * pages leave nothing there for another's (plans/Security-Review-Medium.md SEC-CLI-81). The desktop gives each account
 * a browser profile of its own instead; a webview per account here is a design for later.
 */
object PageStorage {
    private const val APP_ORIGIN = "http://moqui-app.localhost"

    /** Clear the app origin's web storage, on the main thread, where the WebView's storage is reached. */
    @JvmStatic
    fun clear() {
        Handler(Looper.getMainLooper()).post {
            WebStorage.getInstance().deleteOrigin(APP_ORIGIN)
        }
    }
}
