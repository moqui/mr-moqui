// This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License.

package org.moqui.mobile

import android.app.Activity
import android.content.Intent
import android.media.projection.MediaProjectionManager
import android.os.Bundle

/** The system's consent dialog for the capture, each time; no window of its own. */
class ScreenConsentActivity : Activity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        // no touch through another app's window reaches the consent (tapjacking), as on every activity of the app
        ObscuredTouches.refuse(window.decorView)
        if (savedInstanceState == null) {
            val manager = getSystemService(MediaProjectionManager::class.java)
            @Suppress("DEPRECATION")
            startActivityForResult(manager.createScreenCaptureIntent(), 1)
        }
    }

    @Deprecated("The platform's result, as the consent intent gives it")
    override fun onActivityResult(requestCode: Int, resultCode: Int, data: Intent?) {
        @Suppress("DEPRECATION")
        super.onActivityResult(requestCode, resultCode, data)
        ScreenShare.consented(applicationContext, resultCode, data)
        finish()
    }
}
