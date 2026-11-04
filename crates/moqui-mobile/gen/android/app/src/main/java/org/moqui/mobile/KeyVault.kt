// This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License.

package org.moqui.mobile

import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import java.security.KeyStore
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec

/**
 * The app's device keys sealed by an AES-GCM key of the Android Keystore, which never leaves it
 * (crates/moqui-mobile/src/keystore.rs, called from Rust through JNI, src/android.rs). The sealed bytes are the IV's
 * length, the IV and the ciphertext with its tag; Rust keeps them in the app's private files.
 */
object KeyVault {
    private const val ALIAS = "org.moqui.mobile.device-keys"
    private const val TAG_BITS = 128

    private fun key(): SecretKey {
        val store = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }
        (store.getKey(ALIAS, null) as? SecretKey)?.let { return it }
        val generator = KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, "AndroidKeyStore")
        generator.init(
            KeyGenParameterSpec.Builder(ALIAS, KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT)
                .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
                .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
                .setKeySize(256)
                .build()
        )
        return generator.generateKey()
    }

    /** Make or find the Keystore key, so a device that cannot is known before a key is kept. */
    @JvmStatic
    fun ready() {
        key()
    }

    @JvmStatic
    fun seal(plain: ByteArray): ByteArray {
        val cipher = Cipher.getInstance("AES/GCM/NoPadding")
        cipher.init(Cipher.ENCRYPT_MODE, key())
        val iv = cipher.iv
        return byteArrayOf(iv.size.toByte()) + iv + cipher.doFinal(plain)
    }

    @JvmStatic
    fun open(sealed: ByteArray): ByteArray {
        require(sealed.isNotEmpty()) { "nothing sealed" }
        val ivSize = sealed[0].toInt()
        require(ivSize in 12..16 && sealed.size > 1 + ivSize) { "not sealed by this vault" }
        val cipher = Cipher.getInstance("AES/GCM/NoPadding")
        cipher.init(Cipher.DECRYPT_MODE, key(), GCMParameterSpec(TAG_BITS, sealed, 1, ivSize))
        return cipher.doFinal(sealed, 1 + ivSize, sealed.size - 1 - ivSize)
    }
}
