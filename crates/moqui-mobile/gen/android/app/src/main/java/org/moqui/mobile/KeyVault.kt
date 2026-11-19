// This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License.

package org.moqui.mobile

import android.os.Build
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyPermanentlyInvalidatedException
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
 *
 * The device keys' key is not tied to the lock screen: the outbox, an incoming call and a push use the keys while the
 * phone is locked. A second key, for what no background work reads (the chat store passphrases), opens only while the
 * phone is unlocked, from Android 9 (`setUnlockedDeviceRequired`; plans/Security-Review.md SEC-CLI-113): its methods
 * are the `...Unlocked` ones, and before Android 9 `readyUnlocked` refuses, so Rust seals those by the first key.
 */
object KeyVault {
    private const val ALIAS = "org.moqui.mobile.device-keys"
    private const val UNLOCKED_ALIAS = "org.moqui.mobile.unlocked-keys"
    private const val TAG_BITS = 128

    private fun key(alias: String = ALIAS): SecretKey {
        val store = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }
        (store.getKey(alias, null) as? SecretKey)?.let { return it }
        val generator = KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, "AndroidKeyStore")
        val spec = KeyGenParameterSpec.Builder(alias, KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT)
            .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
            .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
            .setKeySize(256)
        if (alias == UNLOCKED_ALIAS && Build.VERSION.SDK_INT >= Build.VERSION_CODES.P) {
            spec.setUnlockedDeviceRequired(true)
        }
        generator.init(spec.build())
        return generator.generateKey()
    }

    /** Make or find the Keystore key, so a device that cannot is known before a key is kept. */
    @JvmStatic
    fun ready() {
        key()
    }

    /** Make or find the key of an unlocked phone; refused before Android 9, which cannot tie a key to the lock. */
    @JvmStatic
    fun readyUnlocked() {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.P) {
            throw UnsupportedOperationException("no key for an unlocked phone before Android 9")
        }
        key(UNLOCKED_ALIAS)
    }

    @JvmStatic
    fun seal(plain: ByteArray): ByteArray = sealBy(ALIAS, plain)

    @JvmStatic
    fun open(sealed: ByteArray): ByteArray = openBy(ALIAS, sealed)

    /**
     * `plain` sealed by the key of an unlocked phone; refused while the phone is locked. A key the Keystore invalidated
     * is made again (what it sealed no longer opens: a passphrase is the server's, and comes back from it).
     */
    @JvmStatic
    fun sealUnlocked(plain: ByteArray): ByteArray = try {
        sealBy(UNLOCKED_ALIAS, plain)
    } catch (err: KeyPermanentlyInvalidatedException) {
        KeyStore.getInstance("AndroidKeyStore").apply { load(null) }.deleteEntry(UNLOCKED_ALIAS)
        sealBy(UNLOCKED_ALIAS, plain)
    }

    /** `sealed` opened by the key of an unlocked phone; refused while the phone is locked. */
    @JvmStatic
    fun openUnlocked(sealed: ByteArray): ByteArray = openBy(UNLOCKED_ALIAS, sealed)

    private fun sealBy(alias: String, plain: ByteArray): ByteArray {
        val cipher = Cipher.getInstance("AES/GCM/NoPadding")
        cipher.init(Cipher.ENCRYPT_MODE, key(alias))
        val iv = cipher.iv
        return byteArrayOf(iv.size.toByte()) + iv + cipher.doFinal(plain)
    }

    private fun openBy(alias: String, sealed: ByteArray): ByteArray {
        require(sealed.isNotEmpty()) { "nothing sealed" }
        val ivSize = sealed[0].toInt()
        require(ivSize in 12..16 && sealed.size > 1 + ivSize) { "not sealed by this vault" }
        val cipher = Cipher.getInstance("AES/GCM/NoPadding")
        cipher.init(Cipher.DECRYPT_MODE, key(alias), GCMParameterSpec(TAG_BITS, sealed, 1, ivSize))
        return cipher.doFinal(sealed, 1 + ivSize, sealed.size - 1 - ivSize)
    }
}
