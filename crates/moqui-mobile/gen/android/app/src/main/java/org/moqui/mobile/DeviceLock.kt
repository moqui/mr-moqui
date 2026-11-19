// This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License.

package org.moqui.mobile

import android.app.KeyguardManager
import android.content.Context

/**
 * Whether the phone has a secure lock screen (a PIN, pattern, password or biometric), which Rust asks by name
 * (src/android.rs device_secure) for the shell's warning: the device keys are sealed by a Keystore key that the lock
 * does not guard, as the outbox and an incoming call need them while the phone is locked, so on a phone with no lock
 * whoever holds it opens every account in the app (plans/Security-Review.md SEC-CLI-113).
 */
object DeviceLock {
    @JvmStatic
    fun isSecure(context: Context): Boolean {
        val keyguard = context.getSystemService(Context.KEYGUARD_SERVICE) as? KeyguardManager ?: return false
        return keyguard.isDeviceSecure
    }
}
