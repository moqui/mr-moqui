// This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License.

package org.moqui.mobile

import android.content.Context
import android.util.Log
import org.unifiedpush.android.connector.data.PublicKeySet
import org.unifiedpush.android.connector.keys.KeyManager

/**
 * The key manager the app hands the connector when it registers an account's instance ([UnifiedPushes.registerAll]):
 * the account's own push keys in Rust (src/push.rs, sealed by the Keystore as the device keys are), so the connector
 * names them to the distributor. The connector's receiver does not use it to open a message (it uses its default key
 * manager, which has no keys of the account); [UnifiedPushService] opens each message with the same keys in Rust
 * (`Native.openFor`). Keys are made only at [generate] (a registration), never for an account signing out.
 */
class RustKeyManager(context: Context) : KeyManager {
    private val app = context.applicationContext

    private fun dir(): String? = if (Native.ensure(app)) Native.dataDir(app) else null

    override fun decrypt(instance: String, sealed: ByteArray): ByteArray? {
        val dir = dir() ?: return null
        return Native.openFor(dir, instance, sealed)
    }

    override fun generate(instance: String) {
        val dir = dir() ?: return
        if (Native.pushPublic(dir, instance, true) == null) Log.w(TAG, "No push keys could be made for $instance")
    }

    override fun getPublicKeySet(instance: String): PublicKeySet? {
        val dir = dir() ?: return null
        val text = Native.pushPublic(dir, instance, false) ?: return null
        val parts = text.split('.')
        if (parts.size != 2) return null
        return PublicKeySet(parts[0], parts[1])
    }

    override fun exists(instance: String): Boolean {
        val dir = dir() ?: return false
        return Native.pushPublic(dir, instance, false) != null
    }

    /** The keys are the account's (its FCM registration may use them too): they go with its sign-out, not here. */
    override fun delete(instance: String) {
        Log.i(TAG, "UnifiedPush let go of $instance; its keys stay until the account signs out")
    }

    private companion object {
        const val TAG = "moqui-push"
    }
}
